//! A deadline guard for Lua's pattern-matching string functions.
//!
//! Lua runs `string.find`, `string.match`, `string.gmatch` and `string.gsub` as single C calls, so
//! the per-instruction timeout hook cannot interrupt them, and a pattern with many backtracking
//! points can run for minutes. Before each call, the guard replays the same match in Rust and polls
//! the deadline. It raises the timeout error when the match cannot finish in half the time left, so
//! the call that follows has the other half. Lua's own functions still produce every result.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
type Instant = tokio::time::Instant;
#[cfg(target_arch = "wasm32")]
type Instant = std::time::Instant;

/// The nesting depth at which Lua refuses a pattern as too complex.
const MAX_MATCH_DEPTH: i32 = 200;
/// The most captures one pattern may open.
const MAX_CAPTURES: usize = 32;
/// Matcher steps between two looks at the clock, for a short pattern. A long pattern makes each
/// step dearer, so it checks the clock sooner.
const STEPS_PER_CLOCK_CHECK: u64 = 1024;
/// Bytes of plain-text search between two looks at the clock.
const PLAIN_BYTES_PER_CLOCK_CHECK: usize = 1 << 20;
/// Marks a capture that is open.
const CAP_UNFINISHED: isize = -1;
/// Marks a position capture, `()`.
const CAP_POSITION: isize = -2;
/// Bytes that make a pattern more than a plain string.
const SPECIALS: &[u8] = b"^$*+?.([%-";

/// Why a dry run ended without an answer.
#[derive(Debug, PartialEq, Eq)]
enum Stop {
    /// The match used up the time it was given.
    TimeLimit,
    /// Lua itself would raise an error here (a malformed pattern, or one nested too deeply), so
    /// the real call reports it.
    Lua,
}

type Step<T> = Result<T, Stop>;

/// Which function a guarded call stands for.
#[derive(Clone, Copy)]
enum Kind {
    Find,
    Match,
    Gsub,
}

/// Replaces Lua's four pattern functions with guarded ones.
pub(super) fn install(lua: &Lua) -> LuaResult<()> {
    let string: LuaTable = lua.globals().get("string")?;
    for (name, kind) in [
        ("find", Kind::Find),
        ("match", Kind::Match),
        ("gsub", Kind::Gsub),
    ] {
        let original: LuaFunction = string.get(name)?;
        let guarded = lua.create_function(move |lua, args: LuaMultiValue| {
            guard_call(lua, kind, &args)?;
            original.call::<LuaMultiValue>(args)
        })?;
        string.set(name, guarded)?;
    }

    let original: LuaFunction = string.get("gmatch")?;
    let guarded = lua.create_function(move |lua, args: LuaMultiValue| {
        let iterator: LuaFunction = original.call(args.clone())?;
        guard_iterator(lua, &args, iterator)
    })?;
    string.set("gmatch", guarded)
}

/// Dry-runs one `find`, `match` or `gsub` call.
fn guard_call(lua: &Lua, kind: Kind, args: &LuaMultiValue) -> LuaResult<()> {
    let Some(limit) = time_limit(lua)? else {
        return Ok(());
    };
    let (Some(subject), Some(pattern)) = (string_arg(lua, args, 0)?, string_arg(lua, args, 1)?)
    else {
        return Ok(());
    };
    let (source, pattern) = (subject.as_bytes(), pattern.as_bytes());

    let outcome = match kind {
        Kind::Find | Kind::Match => {
            let Some(init) = search_start(lua, args.get(2), source.len())? else {
                return Ok(());
            };
            if matches!(kind, Kind::Find)
                && (args.get(3).is_some_and(is_truthy) || has_no_specials(&pattern))
            {
                plain_scan(&source, &pattern, init, Some(limit))
            } else {
                find_scan(&source, &pattern, init, Some(limit)).map(|_| ())
            }
        }
        Kind::Gsub => {
            let max = match args.get(3) {
                None | Some(LuaValue::Nil) => i64::MAX,
                Some(value) => match lua.coerce_integer(value.clone())? {
                    Some(max) => max,
                    None => return Ok(()),
                },
            };
            gsub_scan(&source, &pattern, max, Some(limit)).map(|_| ())
        }
    };
    check(outcome)
}

/// Wraps the iterator that `string.gmatch` returned, so each step is dry-run first.
fn guard_iterator(
    lua: &Lua,
    args: &LuaMultiValue,
    iterator: LuaFunction,
) -> LuaResult<LuaFunction> {
    let (Some(subject), Some(pattern)) = (string_arg(lua, args, 0)?, string_arg(lua, args, 1)?)
    else {
        return Ok(iterator);
    };
    let length = subject.as_bytes().len();
    let first = match args.get(2) {
        None | Some(LuaValue::Nil) => 0,
        Some(value) => match lua.coerce_integer(value.clone())? {
            Some(init) => (relative_start(init, length) - 1).min(length + 1),
            None => return Ok(iterator),
        },
    };
    let state = Rc::new(RefCell::new(GmatchState {
        position: first,
        last_end: None,
    }));
    lua.create_function(move |lua, args: LuaMultiValue| {
        if let Some(limit) = time_limit(lua)? {
            let outcome = gmatch_step(
                &subject.as_bytes(),
                &pattern.as_bytes(),
                &mut state.borrow_mut(),
                Some(limit),
            );
            check(outcome.map(|_| ()))?;
        }
        iterator.call::<LuaMultiValue>(args)
    })
}

/// Raises the timeout error when a dry run ran out of time.
fn check(outcome: Step<()>) -> LuaResult<()> {
    match outcome {
        Err(Stop::TimeLimit) => Err(LuaError::external("execution timeout exceeded")),
        Ok(()) | Err(Stop::Lua) => Ok(()),
    }
}

/// The moment a dry run must stop: half the time left before the command deadline. Returns `None`
/// when the command has no deadline, and the timeout error when it has already passed.
fn time_limit(lua: &Lua) -> LuaResult<Option<Instant>> {
    let Some(deadline) = lua.app_data_ref::<Instant>().map(|deadline| *deadline) else {
        return Ok(None);
    };
    let now = Instant::now();
    if now >= deadline {
        return Err(LuaError::external("execution timeout exceeded"));
    }
    Ok(Some(now + deadline.saturating_duration_since(now) / 2))
}

/// A string or number argument as a Lua string, or `None` when Lua itself would reject it.
fn string_arg(lua: &Lua, args: &LuaMultiValue, index: usize) -> LuaResult<Option<LuaString>> {
    match args.get(index) {
        Some(LuaValue::String(string)) => Ok(Some(string.clone())),
        Some(value @ (LuaValue::Integer(_) | LuaValue::Number(_))) => {
            lua.coerce_string(value.clone())
        }
        _ => Ok(None),
    }
}

fn is_truthy(value: &LuaValue) -> bool {
    !matches!(value, LuaValue::Nil | LuaValue::Boolean(false))
}

/// Whether a pattern holds no special character, which `string.find` searches for as plain text.
fn has_no_specials(pattern: &[u8]) -> bool {
    !pattern.iter().any(|byte| SPECIALS.contains(byte))
}

/// Lua's translation of a 1-based, possibly negative start into a 1-based position.
fn relative_start(position: i64, length: usize) -> usize {
    if position > 0 {
        usize::try_from(position).unwrap_or(usize::MAX)
    } else if position == 0 || position < -i64::try_from(length).unwrap_or(i64::MAX) {
        1
    } else {
        // `position` is in `-length..=-1` here, so the sum stays in `1..=length`.
        usize::try_from(i64::try_from(length).unwrap_or(i64::MAX) + position + 1).unwrap_or(1)
    }
}

/// The 0-based index where a search starts, or `None` when Lua finds nothing or raises an error
/// before it matches.
fn search_start(lua: &Lua, init: Option<&LuaValue>, length: usize) -> LuaResult<Option<usize>> {
    let init = match init {
        None | Some(LuaValue::Nil) => 1,
        Some(value) => match lua.coerce_integer(value.clone())? {
            Some(init) => init,
            None => return Ok(None),
        },
    };
    let start = relative_start(init, length) - 1;
    Ok((start <= length).then_some(start))
}

/// The scan state `string.gmatch` keeps between calls.
struct GmatchState {
    position: usize,
    last_end: Option<usize>,
}

/// Replays the search of `string.find` and `string.match`: the first start at or after `init` where
/// the pattern matches, as `(start, end)` with `end` exclusive.
fn find_scan(
    source: &[u8],
    pattern: &[u8],
    init: usize,
    limit: Option<Instant>,
) -> Step<Option<(usize, usize)>> {
    let (anchored, pattern) = split_anchor(pattern);
    let mut matcher = Matcher::new(source, pattern, limit);
    let mut start = init;
    loop {
        matcher.reset();
        if let Some(end) = matcher.run(start, 0)? {
            return Ok(Some((start, end)));
        }
        if start < source.len() && !anchored {
            start += 1;
        } else {
            return Ok(None);
        }
    }
}

/// Replays the plain-text search of `string.find`. Lua compares the needle at every position that
/// starts with its first byte, so a repetitive subject makes this cost `source.len() * needle.len()`.
/// `init` is at most `source.len()`.
fn plain_scan(source: &[u8], needle: &[u8], init: usize, limit: Option<Instant>) -> Step<()> {
    let Some((first, rest)) = needle.split_first() else {
        return Ok(());
    };
    if needle.len() > source.len() - init {
        return Ok(());
    }
    // The positions where the needle could still start: `start..start + candidates`.
    let mut start = init;
    let mut candidates = source.len() - init - rest.len();
    let mut work = 0;
    while candidates > 0 {
        let Some(offset) = source[start..start + candidates]
            .iter()
            .position(|byte| byte == first)
        else {
            break;
        };
        let hit = start + offset + 1;
        if source[hit..hit + rest.len()] == *rest {
            return Ok(());
        }
        candidates -= hit - start;
        work += hit - start + rest.len();
        start = hit;
        if work >= PLAIN_BYTES_PER_CLOCK_CHECK {
            work = 0;
            if limit.is_some_and(|limit| Instant::now() >= limit) {
                return Err(Stop::TimeLimit);
            }
        }
    }
    Ok(())
}

/// Replays the scan of `string.gsub` and returns how many matches it replaces.
fn gsub_scan(source: &[u8], pattern: &[u8], max: i64, limit: Option<Instant>) -> Step<i64> {
    let (anchored, pattern) = split_anchor(pattern);
    let mut matcher = Matcher::new(source, pattern, limit);
    let (mut position, mut last_end, mut count) = (0, None, 0);
    while count < max {
        matcher.reset();
        let end = matcher.run(position, 0)?;
        if let Some(end) = end.filter(|end| Some(*end) != last_end) {
            count += 1;
            position = end;
            last_end = Some(end);
        } else if position < source.len() {
            position += 1;
        } else {
            break;
        }
        if anchored {
            break;
        }
    }
    Ok(count)
}

/// Replays one call of the iterator from `string.gmatch`, which does not treat `^` as an anchor.
/// Returns whether the call found a match.
fn gmatch_step(
    source: &[u8],
    pattern: &[u8],
    state: &mut GmatchState,
    limit: Option<Instant>,
) -> Step<bool> {
    let mut matcher = Matcher::new(source, pattern, limit);
    let mut position = state.position;
    while position <= source.len() {
        matcher.reset();
        let end = matcher.run(position, 0)?;
        if let Some(end) = end.filter(|end| Some(*end) != state.last_end) {
            state.position = end;
            state.last_end = Some(end);
            return Ok(true);
        }
        position += 1;
    }
    // A scan that finds nothing leaves the state alone, as Lua's iterator does.
    Ok(false)
}

/// Splits a leading `^` off a pattern.
fn split_anchor(pattern: &[u8]) -> (bool, &[u8]) {
    match pattern.split_first() {
        Some((b'^', rest)) => (true, rest),
        _ => (false, pattern),
    }
}

/// A port of the matcher in Lua 5.4's `lstrlib.c`. It counts its steps and looks at the clock every
/// so often, which Lua's own matcher never does. Positions are byte indexes, and a read past the
/// end gives `0`, as Lua's terminating NUL does.
struct Matcher<'a> {
    source: &'a [u8],
    pattern: &'a [u8],
    depth: i32,
    level: usize,
    captures: [(usize, isize); MAX_CAPTURES],
    steps: u64,
    clock_interval: u64,
    limit: Option<Instant>,
}

impl<'a> Matcher<'a> {
    fn new(source: &'a [u8], pattern: &'a [u8], limit: Option<Instant>) -> Self {
        Self {
            source,
            pattern,
            depth: MAX_MATCH_DEPTH,
            level: 0,
            captures: [(0, 0); MAX_CAPTURES],
            steps: 0,
            clock_interval: (STEPS_PER_CLOCK_CHECK / (1 + pattern.len() as u64 / 256)).max(1),
            limit,
        }
    }

    /// Clears the captures and the nesting depth before a new attempt, keeping the step count.
    fn reset(&mut self) {
        self.level = 0;
        self.depth = MAX_MATCH_DEPTH;
    }

    fn pattern_at(&self, index: usize) -> u8 {
        self.pattern.get(index).copied().unwrap_or(0)
    }

    fn source_at(&self, index: usize) -> u8 {
        self.source.get(index).copied().unwrap_or(0)
    }

    /// Counts one step and looks at the clock every `clock_interval` steps.
    fn tick(&mut self) -> Step<()> {
        self.steps += 1;
        if self.steps.is_multiple_of(self.clock_interval)
            && self.limit.is_some_and(|limit| Instant::now() >= limit)
        {
            return Err(Stop::TimeLimit);
        }
        Ok(())
    }

    /// The index just past the single-character class that starts at `start`.
    fn class_end(&self, start: usize) -> Step<usize> {
        let mut index = start;
        let first = self.pattern_at(index);
        index += 1;
        match first {
            b'%' => {
                if index == self.pattern.len() {
                    return Err(Stop::Lua);
                }
                Ok(index + 1)
            }
            b'[' => {
                if self.pattern_at(index) == b'^' {
                    index += 1;
                }
                loop {
                    if index == self.pattern.len() {
                        return Err(Stop::Lua);
                    }
                    let byte = self.pattern_at(index);
                    index += 1;
                    if byte == b'%' && index < self.pattern.len() {
                        index += 1;
                    }
                    if self.pattern_at(index) == b']' {
                        return Ok(index + 1);
                    }
                }
            }
            _ => Ok(index),
        }
    }

    /// Whether `byte` is in the set `[...]` that starts at `start` and whose `]` is at `end`.
    fn in_set(&self, byte: u8, start: usize, end: usize) -> bool {
        let mut index = start;
        let mut found = true;
        if self.pattern_at(index + 1) == b'^' {
            found = false;
            index += 1;
        }
        loop {
            index += 1;
            if index >= end {
                return !found;
            }
            if self.pattern_at(index) == b'%' {
                index += 1;
                if class_matches(byte, self.pattern_at(index)) {
                    return found;
                }
            } else if self.pattern_at(index + 1) == b'-' && index + 2 < end {
                index += 2;
                if self.pattern_at(index - 2) <= byte && byte <= self.pattern_at(index) {
                    return found;
                }
            } else if self.pattern_at(index) == byte {
                return found;
            }
        }
    }

    /// Whether the source byte at `at` matches the class `pattern[start..end]`.
    fn single(&self, at: usize, start: usize, end: usize) -> bool {
        let Some(&byte) = self.source.get(at) else {
            return false;
        };
        match self.pattern_at(start) {
            b'.' => true,
            b'%' => class_matches(byte, self.pattern_at(start + 1)),
            b'[' => self.in_set(byte, start, end - 1),
            literal => literal == byte,
        }
    }

    /// `%bxy`: the end of the balanced run that starts at `at`, or `None`.
    fn balanced(&self, at: usize, args: usize) -> Step<Option<usize>> {
        if args + 1 >= self.pattern.len() {
            return Err(Stop::Lua);
        }
        if self.source_at(at) != self.pattern_at(args) {
            return Ok(None);
        }
        let (open, close) = (self.pattern_at(args), self.pattern_at(args + 1));
        let mut depth = 1;
        let mut index = at;
        loop {
            index += 1;
            let Some(&byte) = self.source.get(index) else {
                return Ok(None);
            };
            if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(index + 1));
                }
            } else if byte == open {
                depth += 1;
            }
        }
    }

    /// `x*` and `x+`: take as many repetitions as match, then give them back one at a time.
    fn max_expand(&mut self, at: usize, start: usize, end: usize) -> Step<Option<usize>> {
        let mut count = 0;
        while self.single(at + count, start, end) {
            count += 1;
            self.tick()?;
        }
        loop {
            if let Some(found) = self.run(at + count, end + 1)? {
                return Ok(Some(found));
            }
            if count == 0 {
                return Ok(None);
            }
            count -= 1;
        }
    }

    /// `x-`: take as few repetitions as match.
    fn min_expand(&mut self, mut at: usize, start: usize, end: usize) -> Step<Option<usize>> {
        loop {
            if let Some(found) = self.run(at, end + 1)? {
                return Ok(Some(found));
            }
            if self.single(at, start, end) {
                at += 1;
            } else {
                return Ok(None);
            }
        }
    }

    fn start_capture(&mut self, at: usize, next: usize, what: isize) -> Step<Option<usize>> {
        if self.level >= MAX_CAPTURES {
            return Err(Stop::Lua);
        }
        self.captures[self.level] = (at, what);
        self.level += 1;
        let found = self.run(at, next)?;
        if found.is_none() {
            self.level -= 1;
        }
        Ok(found)
    }

    fn end_capture(&mut self, at: usize, next: usize) -> Step<Option<usize>> {
        let open = (0..self.level)
            .rev()
            .find(|&index| self.captures[index].1 == CAP_UNFINISHED)
            .ok_or(Stop::Lua)?;
        let length = isize::try_from(at - self.captures[open].0).map_err(|_| Stop::Lua)?;
        self.captures[open].1 = length;
        let found = self.run(at, next)?;
        if found.is_none() {
            self.captures[open].1 = CAP_UNFINISHED;
        }
        Ok(found)
    }

    /// `%1` to `%9`: the text of an earlier capture, matched again.
    fn back_reference(&self, at: usize, digit: u8) -> Step<Option<usize>> {
        let index = usize::from(digit.wrapping_sub(b'1'));
        if digit < b'1' || index >= self.level || self.captures[index].1 == CAP_UNFINISHED {
            return Err(Stop::Lua);
        }
        let (init, length) = self.captures[index];
        let Ok(length) = usize::try_from(length) else {
            return Ok(None);
        };
        let remaining = self.source.len() - at;
        if remaining >= length && self.source[init..init + length] == self.source[at..at + length] {
            Ok(Some(at + length))
        } else {
            Ok(None)
        }
    }

    /// Matches `pattern[start..]` against the source from `at`, and returns where the match ends.
    fn run(&mut self, mut at: usize, mut start: usize) -> Step<Option<usize>> {
        if self.depth == 0 {
            return Err(Stop::Lua);
        }
        self.depth -= 1;
        let found = loop {
            self.tick()?;
            if start == self.pattern.len() {
                break Some(at);
            }
            match self.pattern_at(start) {
                b'(' => {
                    break if self.pattern_at(start + 1) == b')' {
                        self.start_capture(at, start + 2, CAP_POSITION)?
                    } else {
                        self.start_capture(at, start + 1, CAP_UNFINISHED)?
                    };
                }
                b')' => break self.end_capture(at, start + 1)?,
                b'$' if start + 1 == self.pattern.len() => {
                    break (at == self.source.len()).then_some(at);
                }
                b'%' if self.pattern_at(start + 1) == b'b' => {
                    match self.balanced(at, start + 2)? {
                        Some(end) => {
                            at = end;
                            start += 4;
                        }
                        None => break None,
                    }
                }
                b'%' if self.pattern_at(start + 1) == b'f' => {
                    start += 2;
                    if self.pattern_at(start) != b'[' {
                        return Err(Stop::Lua);
                    }
                    let end = self.class_end(start)?;
                    let previous = if at == 0 { 0 } else { self.source[at - 1] };
                    if !self.in_set(previous, start, end - 1)
                        && self.in_set(self.source_at(at), start, end - 1)
                    {
                        start = end;
                    } else {
                        break None;
                    }
                }
                b'%' if self.pattern_at(start + 1).is_ascii_digit() => {
                    match self.back_reference(at, self.pattern_at(start + 1))? {
                        Some(end) => {
                            at = end;
                            start += 2;
                        }
                        None => break None,
                    }
                }
                _ => {
                    let end = self.class_end(start)?;
                    let suffix = self.pattern_at(end);
                    if !self.single(at, start, end) {
                        if matches!(suffix, b'*' | b'?' | b'-') {
                            start = end + 1;
                        } else {
                            break None;
                        }
                    } else {
                        match suffix {
                            b'?' => {
                                if let Some(found) = self.run(at + 1, end + 1)? {
                                    break Some(found);
                                }
                                start = end + 1;
                            }
                            b'+' => break self.max_expand(at + 1, start, end)?,
                            b'*' => break self.max_expand(at, start, end)?,
                            b'-' => break self.min_expand(at, start, end)?,
                            _ => {
                                at += 1;
                                start = end;
                            }
                        }
                    }
                }
            }
        };
        self.depth += 1;
        Ok(found)
    }
}

/// Whether `byte` is in the class named by the letter `class`, as in `%a`, with C-locale rules.
fn class_matches(byte: u8, class: u8) -> bool {
    let found = match class.to_ascii_lowercase() {
        b'a' => byte.is_ascii_alphabetic(),
        b'c' => byte.is_ascii_control(),
        b'd' => byte.is_ascii_digit(),
        b'g' => byte.is_ascii_graphic(),
        b'l' => byte.is_ascii_lowercase(),
        b'p' => byte.is_ascii_punctuation(),
        b's' => matches!(byte, b' ' | 0x09..=0x0d),
        b'u' => byte.is_ascii_uppercase(),
        b'w' => byte.is_ascii_alphanumeric(),
        b'x' => byte.is_ascii_hexdigit(),
        b'z' => byte == 0,
        _ => return class == byte,
    };
    if class.is_ascii_uppercase() {
        !found
    } else {
        found
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A small deterministic generator, so a failure reproduces.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % bound as u64).unwrap()
        }
    }

    fn random_pattern(random: &mut Random) -> Vec<u8> {
        const ITEMS: &[&str] = &[
            "a", "b", "1", " ", ".", "%a", "%d", "%s", "%w", "%A", "[ab]", "[^a]", "[a-c]",
            "[%d ]", "%.", "%%", "(", ")", "()", "%b()", "%f[%w]", "%1", "%2", "%z", "[%z]", "%Z",
        ];
        const SUFFIXES: &[&str] = &["", "", "", "*", "+", "-", "?"];
        const BROKEN: &[&str] = &["%", "[a", "%b", "%f", "[^", "[%", "%f[", "(("];
        let mut pattern = String::new();
        if random.below(6) == 0 {
            pattern.push('^');
        }
        for _ in 0..random.below(7) {
            pattern += ITEMS[random.below(ITEMS.len())];
            pattern += SUFFIXES[random.below(SUFFIXES.len())];
        }
        if random.below(6) == 0 {
            pattern.push('$');
        }
        if random.below(12) == 0 {
            pattern += BROKEN[random.below(BROKEN.len())];
        }
        pattern.into_bytes()
    }

    fn random_subject(random: &mut Random) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ab1 .()%\0";
        (0..random.below(11))
            .map(|_| ALPHABET[random.below(ALPHABET.len())])
            .collect()
    }

    fn lua_string_function(lua: &Lua, name: &str) -> LuaFunction {
        lua.globals()
            .get::<LuaTable>("string")
            .unwrap()
            .get(name)
            .unwrap()
    }

    /// Lua's own answer for `string.find`: `Some((start, end))` as the guard reports it, `None` for
    /// no match, and the message when Lua raises an error.
    fn lua_find(
        lua: &Lua,
        source: &[u8],
        pattern: &[u8],
        init: i64,
    ) -> Result<Option<(usize, usize)>, String> {
        let find = lua_string_function(lua, "find");
        let args = (
            lua.create_string(source).unwrap(),
            lua.create_string(pattern).unwrap(),
            init,
        );
        match find.call::<LuaMultiValue>(args) {
            Err(error) => Err(error.to_string()),
            Ok(values) => match (values.front(), values.get(1)) {
                (Some(LuaValue::Integer(start)), Some(LuaValue::Integer(end))) => Ok(Some((
                    usize::try_from(*start).unwrap() - 1,
                    usize::try_from(*end).unwrap(),
                ))),
                _ => Ok(None),
            },
        }
    }

    #[test]
    fn find_agrees_with_lua_on_random_patterns() {
        let lua = Lua::new();
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        let (mut matched, mut missed, mut errors, mut late_errors) = (0, 0, 0, 0);
        for _ in 0..60_000 {
            let (source, pattern) = (random_subject(&mut random), random_pattern(&mut random));
            let init = i64::try_from(random.below(source.len() + 2)).unwrap();
            if has_no_specials(&pattern) {
                // `string.find` searches for these as plain text, so the guard never sees them.
                continue;
            }
            let expected = lua_find(&lua, &source, &pattern, init);
            let start = usize::try_from(init.max(1)).unwrap() - 1;
            let actual = if start > source.len() {
                continue;
            } else {
                find_scan(&source, &pattern, start, None)
            };
            let case = format!(
                "find({:?}, {:?}, {init})",
                String::from_utf8_lossy(&source),
                String::from_utf8_lossy(&pattern)
            );
            match (expected, actual) {
                (Ok(Some(lua_span)), Ok(Some(span))) => {
                    assert_eq!(span, lua_span, "{case}");
                    matched += 1;
                }
                (Ok(None), Ok(None)) => missed += 1,
                (Err(_), Err(Stop::Lua)) => errors += 1,
                // Lua reports an open capture only after the match, when it builds the results, so
                // the guard has already seen the whole match by then.
                (Err(message), Ok(_)) if message.contains("unfinished capture") => late_errors += 1,
                (expected, actual) => panic!("{case}: Lua gave {expected:?}, the guard {actual:?}"),
            }
        }
        // The corpus has to exercise every outcome, or the test proves nothing.
        assert!(
            matched > 3_000 && missed > 3_000 && errors > 500 && late_errors > 50,
            "{matched} {missed} {errors} {late_errors}"
        );
    }

    #[test]
    fn gsub_and_gmatch_agree_with_lua_on_random_patterns() {
        let lua = Lua::new();
        let gsub = lua_string_function(&lua, "gsub");
        let count_matches: LuaFunction = lua
            .load("local n = 0 for _ in string.gmatch(...) do n = n + 1 end return n")
            .into_function()
            .unwrap();
        let mut random = Random(0xD1B5_4A32_D192_ED03);
        let (mut counted, mut errors) = (0, 0);
        for _ in 0..40_000 {
            let (source, pattern) = (random_subject(&mut random), random_pattern(&mut random));
            let args = (
                lua.create_string(&source).unwrap(),
                lua.create_string(&pattern).unwrap(),
            );
            let case = format!(
                "({:?}, {:?})",
                String::from_utf8_lossy(&source),
                String::from_utf8_lossy(&pattern)
            );

            let expected = gsub
                .call::<(LuaValue, i64)>((args.0.clone(), args.1.clone(), "x"))
                .map(|(_, count)| count);
            match (expected, gsub_scan(&source, &pattern, i64::MAX, None)) {
                (Ok(lua_count), Ok(count)) => {
                    assert_eq!(count, lua_count, "gsub{case}");
                    counted += 1;
                }
                (Err(_), Err(Stop::Lua)) => errors += 1,
                (expected, actual) => {
                    panic!("gsub{case}: Lua gave {expected:?}, the guard {actual:?}")
                }
            }

            let expected = count_matches.call::<i64>(args);
            let mut state = GmatchState {
                position: 0,
                last_end: None,
            };
            let mut count = 0;
            let actual = loop {
                match gmatch_step(&source, &pattern, &mut state, None) {
                    Ok(true) => count += 1,
                    Ok(false) => break Ok(count),
                    Err(stop) => break Err(stop),
                }
            };
            match (expected, actual) {
                (Ok(lua_count), Ok(count)) => assert_eq!(count, lua_count, "gmatch{case}"),
                (Err(_), Err(Stop::Lua)) => {}
                (Err(error), Ok(_)) if error.to_string().contains("unfinished capture") => {}
                (expected, actual) => {
                    panic!("gmatch{case}: Lua gave {expected:?}, the guard {actual:?}")
                }
            }
        }
        assert!(counted > 3_000 && errors > 300, "{counted} {errors}");
    }

    #[test]
    fn a_backtracking_match_stops_at_the_time_limit() {
        let source = vec![b'a'; 30];
        let pattern = [b"a?".repeat(30), source.clone()].concat();
        let started = std::time::Instant::now();
        let limit = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            find_scan(&source, &pattern, 0, Some(limit)),
            Err(Stop::TimeLimit)
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_polynomial_match_stops_at_the_time_limit() {
        let source = vec![b'a'; 4000];
        let started = std::time::Instant::now();
        let limit = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            find_scan(&source, b".-.-.-x", 0, Some(limit)),
            Err(Stop::TimeLimit)
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_plain_search_stops_at_the_time_limit() {
        let source = vec![b'a'; 600_000];
        let needle = [vec![b'a'; 300_000], vec![b'b']].concat();
        let started = std::time::Instant::now();
        let limit = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            plain_scan(&source, &needle, 0, Some(limit)),
            Err(Stop::TimeLimit)
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );

        let limit = Instant::now() + Duration::from_secs(30);
        assert_eq!(plain_scan(b"hello world", b"o w", 0, Some(limit)), Ok(()));
        assert_eq!(plain_scan(b"hello", b"", 0, Some(limit)), Ok(()));
        assert_eq!(plain_scan(b"hello", b"hello!", 0, Some(limit)), Ok(()));
        assert_eq!(plain_scan(b"hello", b"o", 5, Some(limit)), Ok(()));
    }

    #[test]
    fn a_class_that_only_old_lua_names_still_takes_the_same_path() {
        let source = vec![0u8; 30];
        let pattern = [b"%z?".repeat(30), b"%z".repeat(30), b"x".to_vec()].concat();
        let limit = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            find_scan(&source, &pattern, 0, Some(limit)),
            Err(Stop::TimeLimit)
        );
    }

    #[test]
    fn a_gmatch_scan_that_finds_nothing_leaves_its_state_alone() {
        let mut state = GmatchState {
            position: 2,
            last_end: None,
        };
        assert_eq!(gmatch_step(b"abcabc", b"x", &mut state, None), Ok(false));
        assert_eq!((state.position, state.last_end), (2, None));
        assert_eq!(gmatch_step(b"abcabc", b"c", &mut state, None), Ok(true));
        assert_eq!((state.position, state.last_end), (3, Some(3)));
    }

    #[test]
    fn a_cheap_match_finishes_inside_a_tight_limit() {
        let source = b"  hello world  ".repeat(20_000);
        let limit = Instant::now() + Duration::from_secs(30);
        assert_eq!(
            find_scan(&source, b"^%s*(.-)%s*$", 0, Some(limit)).map(|m| m.is_some()),
            Ok(true)
        );
        assert_eq!(
            gsub_scan(&source, b"%s+", i64::MAX, Some(limit)),
            Ok(40_001)
        );
    }
}
