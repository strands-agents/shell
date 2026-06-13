use std::cell::RefCell;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use crate::builtins;
use crate::commands;
use crate::io::{CURRENT_KERNEL, CURRENT_PROCESS};
use crate::os::{self, FdReader, Kernel, OpenFlags, Process, STDERR, STDIN, STDOUT};
use crate::parser::{self, Connector, Item, Redirect, Word, WordPart};

/// Signals for break/continue/exit control flow.
#[derive(Debug)]
enum ControlFlow {
    Break(i32),
    Continue(i32),
    Return(i32),
    Exit(i32),
}

fn is_special(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "continue"
            | "exit"
            | "return"
            | "eval"
            | "exec"
            | "."
            | ":"
            | "set"
            | "shift"
            | "export"
            | "readonly"
            | "trap"
            | "unset"
    )
}

async fn find_in_path(os: &dyn Kernel, proc: &Process, name: &str) -> Option<String> {
    let path_var = proc.env.get("PATH")?;
    for dir in path_var.split(':') {
        let full = if dir.is_empty() {
            format!("./{name}")
        } else {
            format!("{dir}/{name}")
        };
        if os.is_executable(proc, &full).await {
            return Some(full);
        }
    }
    None
}

/// Resolve an executable path to either a multicall command name or a shebang interpreter + script.
enum ExecTarget {
    /// Multicall: the invocation basename maps to a builtin/command
    Multicall(String),
    /// Shebang: interpreter args + script path
    Shebang(Vec<String>),
}

async fn resolve_executable(
    os: &dyn Kernel,
    proc: &Process,
    name: &str,
) -> Option<(String, ExecTarget)> {
    // Find the file
    let found = if name.contains('/') {
        if os.is_executable(proc, name).await {
            Some(name.to_string())
        } else {
            None
        }
    } else {
        find_in_path(os, proc, name).await
    }?;

    // Resolve symlinks to get the real path
    let canonical = os.canonicalize(proc, &found).await.ok()?;
    let canon_str = canonical.to_string_lossy();

    // Check for multicall binary (lash)
    if canon_str.ends_with("/lash") {
        let basename = found.rsplit('/').next().unwrap_or(&found).to_string();
        return Some((found, ExecTarget::Multicall(basename)));
    }

    // Read first line to check for shebang
    let mut child = proc.fork();
    let fd = os.open(&mut child, &found, OpenFlags::read()).await.ok()?;
    let mut reader = child.take_reader(fd).ok()?;
    let mut buf = [0u8; 256];
    use tokio::io::AsyncReadExt;
    let n = reader.read(&mut buf).await.ok()?;
    if n >= 2 && buf[0] == b'#' && buf[1] == b'!' {
        let line_end = buf[..n].iter().position(|&b| b == b'\n').unwrap_or(n);
        let shebang = std::str::from_utf8(&buf[2..line_end]).ok()?.trim();
        let mut parts: Vec<String> = shebang.split_whitespace().map(String::from).collect();
        if parts.is_empty() {
            return None;
        }
        parts.push(found.clone());
        return Some((found, ExecTarget::Shebang(parts)));
    }

    None
}

/// Read a script file and execute it via execute_sourced in a child context.
async fn run_script(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    script_path: &str,
    args: &[String],
) -> i32 {
    let fd = match os.open(proc, script_path, OpenFlags::read()).await {
        Ok(fd) => fd,
        Err(e) => {
            proc.err_msg(&format!("strands-shell: {script_path}: {e}"));
            return 126;
        }
    };
    let mut reader = match proc.take_reader(fd) {
        Ok(r) => r,
        Err(e) => {
            proc.err_msg(&format!("strands-shell: {script_path}: {e}"));
            return 126;
        }
    };
    let content = match os::read_to_string_limited(&mut reader, proc.max_output).await {
        Ok(s) => s,
        Err(e) => {
            proc.err_msg(&format!("strands-shell: {script_path}: {e}"));
            return 126;
        }
    };
    let saved_args = std::mem::replace(&mut proc.args, args.to_vec());
    let saved_arg0 = std::mem::replace(&mut proc.arg0, script_path.to_string());
    proc.depth += 1;
    let (exit, _) = execute_sourced(os, proc, &content).await;
    proc.depth -= 1;
    proc.args = saved_args;
    proc.arg0 = saved_arg0;
    exit
}

/// Execute a full input line. `proc` is the shell's process — cd modifies it.
/// Returns (exit_code, should_exit).
pub async fn execute(os: Arc<dyn Kernel>, proc: &mut Process, input: &str) -> (i32, bool) {
    let result = if input.contains('\n') {
        execute_sourced(os.clone(), proc, input).await
    } else {
        execute_with_reader(os.clone(), proc, input, &mut |_| None).await
    };
    // Run EXIT trap if one is set (only at top-level depth)
    if proc.depth == 0
        && let Some(cmd) = proc.traps.remove("EXIT")
    {
        let _ = Box::pin(execute_with_reader(os, proc, &cmd, &mut |_| None)).await;
    }
    result
}

/// Execute and capture all stdout/stderr output. Returns (exit_code, stdout, stderr).
pub async fn execute_capture(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    input: &str,
) -> (i32, String, String) {
    proc.capture = true;
    proc.captured_output.clear();
    proc.captured_stderr.clear();
    let (code, _) = execute(os, proc, input).await;
    proc.capture = false;
    let stdout = std::mem::take(&mut proc.captured_output);
    let stderr = std::mem::take(&mut proc.captured_stderr);
    (code, stdout, stderr)
}

/// Execute with a line reader for here-documents.
pub async fn execute_with_reader(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    input: &str,
    read_line: &mut dyn FnMut(&str) -> Option<String>,
) -> (i32, bool) {
    if proc.max_input > 0 && input.len() > proc.max_input {
        proc.err_msg("strands-shell: input too large");
        return (1, false);
    }
    let command_line = match parser::parse_with_aliases(input, read_line, &proc.aliases) {
        Ok(p) => p,
        Err(e) => {
            proc.err_msg(&format!("strands-shell: {e}"));
            return (1, false);
        }
    };

    match execute_command_line_inner(os, proc, &command_line).await {
        Ok(code) => {
            proc.last_exit = code;
            (code, false)
        }
        Err(ControlFlow::Exit(code)) => {
            proc.last_exit = code;
            (code, true)
        }
        Err(ControlFlow::Return(code)) => {
            proc.last_exit = code;
            (code, false)
        }
        Err(_) => (proc.last_exit, false), // break/continue at top level = no-op
    }
}

/// Execute sourced file content incrementally, so aliases defined by earlier
/// commands are visible when parsing later commands.
pub async fn execute_sourced(os: Arc<dyn Kernel>, proc: &mut Process, input: &str) -> (i32, bool) {
    let mut last_code = 0i32;
    let mut accum = String::new();
    let mut last_err = String::new();
    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        if accum.is_empty() {
            accum = lines[i].to_string();
        } else {
            accum.push('\n');
            accum.push_str(lines[i]);
        }
        i += 1;

        let mut line_idx = i;
        let mut reader = |_delim: &str| -> Option<String> {
            if line_idx < lines.len() {
                let line = lines[line_idx].to_string();
                line_idx += 1;
                Some(line)
            } else {
                None
            }
        };

        match parser::parse_with_aliases(&accum, &mut reader, &proc.aliases) {
            Ok(cl) => {
                i = line_idx; // advance past any lines consumed by heredoc
                accum.clear();
                last_err.clear();
                if cl.is_empty() {
                    continue;
                }
                match execute_command_line_inner(os.clone(), proc, &cl).await {
                    Ok(code) => {
                        proc.last_exit = code;
                        last_code = code;
                    }
                    Err(ControlFlow::Exit(code)) => {
                        proc.last_exit = code;
                        return (code, true);
                    }
                    Err(ControlFlow::Return(code)) => {
                        proc.last_exit = code;
                        return (code, false);
                    }
                    Err(_) => {
                        last_code = proc.last_exit;
                    }
                }
            }
            Err(e) => {
                last_err = e;
                continue;
            }
        }
    }

    if !accum.is_empty() && !last_err.is_empty() {
        proc.err_msg(&format!("strands-shell: {last_err}"));
        return (1, false);
    }

    (last_code, false)
}

/// Expand a Word into a String using the current environment.
async fn expand_word(os: Arc<dyn Kernel>, proc: &mut Process, word: &Word) -> String {
    let mut result = String::new();
    for part in word {
        expand_part(os.clone(), proc, part, &mut result).await;
    }
    result
}

/// A segment being built during word expansion. "$@" introduces split points
/// between segments; each segment is IFS-split independently.
struct Segment {
    buf: String,
    splittable: Vec<bool>, // parallel to buf bytes
    globbable: Vec<bool>,  // parallel to buf bytes — false = quoted (no glob)
    has_nonsplit: bool,
}

impl Segment {
    fn new() -> Self {
        Self {
            buf: String::new(),
            splittable: Vec::new(),
            globbable: Vec::new(),
            has_nonsplit: false,
        }
    }
    fn push(&mut self, s: &str, splittable: bool, globbable: bool) {
        self.buf.push_str(s);
        let new_len = self.buf.len();
        self.splittable.resize(new_len, splittable);
        self.globbable.resize(new_len, globbable);
        if !splittable {
            self.has_nonsplit = true;
        }
    }
}

/// Expand a word and perform IFS field splitting on unquoted expansion results.
/// Handles "$@" (separate fields per positional param) and "$*" (join with IFS[0]).
/// After IFS splitting, applies pathname globbing on unquoted metacharacters.
async fn expand_word_split(os: Arc<dyn Kernel>, proc: &mut Process, word: &Word) -> Vec<String> {
    let ifs = proc
        .env
        .get("IFS")
        .cloned()
        .unwrap_or_else(|| " \t\n".into());
    let mut segments: Vec<Segment> = vec![Segment::new()];

    for part in word {
        expand_part_split(os.clone(), proc, part, &mut segments, &ifs, false).await;
    }

    let mut result = Vec::new();
    for seg in segments {
        let fields = ifs_split(seg, &ifs);
        for (f, do_glob) in fields {
            if do_glob {
                glob_expand(os.as_ref(), proc, &f, &mut result).await;
            } else {
                result.push(f);
            }
        }
    }
    result
}

/// Expand a single WordPart into segments, handling "$@" splitting.
/// `quoted` is true when inside a DoubleQuoted context.
fn expand_part_split<'a>(
    os: Arc<dyn Kernel>,
    proc: &'a mut Process,
    part: &'a WordPart,
    segments: &'a mut Vec<Segment>,
    ifs: &'a str,
    quoted: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    Box::pin(async move {
        match part {
            WordPart::Literal(s) => {
                // Unquoted literals are globbable; quoted literals are not
                segments.last_mut().unwrap().push(s, false, !quoted);
            }
            WordPart::SingleQuoted(s) => {
                segments.last_mut().unwrap().push(s, false, false);
            }
            WordPart::Var(name) if name == "@" => {
                if quoted {
                    // "$@" — each arg becomes a separate segment
                    let args = proc.args.clone();
                    for (i, arg) in args.iter().enumerate() {
                        segments.last_mut().unwrap().push(arg, false, false);
                        if i + 1 < args.len() {
                            segments.push(Segment::new());
                        }
                    }
                } else {
                    // Unquoted $@ — each arg separate, but splittable
                    let args = proc.args.clone();
                    for (i, arg) in args.iter().enumerate() {
                        segments.last_mut().unwrap().push(arg, true, false);
                        if i + 1 < args.len() {
                            segments.push(Segment::new());
                        }
                    }
                }
            }
            WordPart::Var(name) if name == "*" => {
                if quoted {
                    // "$*" — join with IFS[0] (empty string if IFS is empty)
                    let sep = ifs.chars().next().map_or(String::new(), |c| c.to_string());
                    let joined = proc.args.join(&sep);
                    segments.last_mut().unwrap().push(&joined, false, false);
                } else {
                    // Unquoted $* — join with space, mark splittable
                    let joined = proc.args.join(" ");
                    segments.last_mut().unwrap().push(&joined, true, false);
                }
            }
            WordPart::Var(name) => {
                if check_nounset(proc, name) {
                    return;
                }
                if let Some(val) = resolve_var(proc, name) {
                    let splittable = !quoted;
                    segments.last_mut().unwrap().push(&val, splittable, false);
                }
            }
            WordPart::VarOp(..)
            | WordPart::Backtick(_)
            | WordPart::DollarParen(_)
            | WordPart::Arith(_) => {
                let mut tmp = String::new();
                expand_part(os, proc, part, &mut tmp).await;
                let splittable = !quoted;
                segments.last_mut().unwrap().push(&tmp, splittable, false);
            }
            WordPart::Tilde(_) => {
                let mut tmp = String::new();
                expand_part(os, proc, part, &mut tmp).await;
                segments.last_mut().unwrap().push(&tmp, false, false);
            }
            WordPart::DoubleQuoted(parts) => {
                for p in parts {
                    expand_part_split(os.clone(), proc, p, segments, ifs, true).await;
                }
            }
        }
    })
}

/// Perform IFS field splitting on a single segment.
fn ifs_split(seg: Segment, ifs: &str) -> Vec<(String, bool)> {
    let Segment {
        buf,
        splittable,
        globbable,
        has_nonsplit,
    } = seg;

    // Check if any glob metachar is in a globbable position
    let has_glob = buf
        .bytes()
        .zip(globbable.iter())
        .any(|(b, &g)| g && (b == b'*' || b == b'?' || b == b'['));

    if buf.is_empty() {
        return if has_nonsplit {
            vec![(buf, false)]
        } else {
            vec![]
        };
    }

    if !splittable.iter().any(|&s| s) {
        return vec![(buf, has_glob)];
    }

    if ifs.is_empty() {
        return vec![(buf, has_glob)];
    }

    let ifs_ws: Vec<char> = ifs.chars().filter(|c| " \t\n".contains(*c)).collect();
    let chars: Vec<char> = buf.chars().collect();
    let mut char_byte_offsets = Vec::with_capacity(chars.len());
    let mut byte_off = 0;
    for &ch in &chars {
        char_byte_offsets.push(byte_off);
        byte_off += ch.len_utf8();
    }

    let is_ifs = |ch: char| ifs.contains(ch);
    let is_ifs_ws = |ch: char| ifs_ws.contains(&ch);
    let is_splittable_at = |ci: usize| splittable[char_byte_offsets[ci]];

    let mut fields: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0;

    while i < chars.len() && is_splittable_at(i) && is_ifs_ws(chars[i]) {
        i += 1;
    }

    while i < chars.len() {
        if is_splittable_at(i) && is_ifs(chars[i]) {
            fields.push(std::mem::take(&mut current));
            while i < chars.len() && is_splittable_at(i) && is_ifs_ws(chars[i]) {
                i += 1;
            }
            if i < chars.len() && is_splittable_at(i) && !is_ifs_ws(chars[i]) && is_ifs(chars[i]) {
                i += 1;
                while i < chars.len() && is_splittable_at(i) && is_ifs_ws(chars[i]) {
                    i += 1;
                }
            }
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }

    if !current.is_empty() || fields.is_empty() {
        fields.push(current);
    }

    if fields.len() == 1 && fields[0].is_empty() && !has_nonsplit {
        return vec![];
    }

    // After IFS split, each resulting field inherits globbability.
    // Fields from splittable expansions also get globbed (e.g., $PAT where PAT="*.txt").
    let do_glob = has_glob || splittable.iter().any(|&s| s);
    fields
        .into_iter()
        .map(|f| {
            let fg = do_glob && (f.contains('*') || f.contains('?') || f.contains('['));
            (f, fg)
        })
        .collect()
}

/// Expand glob metacharacters in a field using the Kernel abstraction.
/// If matches are found, add them sorted; otherwise add the original field unchanged.
async fn glob_expand(os: &dyn Kernel, proc: &Process, field: &str, result: &mut Vec<String>) {
    let matches = os.glob(proc, field).await;
    if matches.is_empty() {
        result.push(field.to_string());
    } else {
        result.extend(matches);
    }
}

/// Pre-expand $-prefixed references in an arithmetic expression.
/// Bare variable names (without $) are left for ArithParser to resolve.
fn expand_arith_expr<'a>(
    os: Arc<dyn Kernel>,
    proc: &'a mut Process,
    expr: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + 'a>> {
    Box::pin(async move {
        let mut result = String::new();
        let mut chars = expr.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c == '$' {
                chars.next();
                match chars.peek() {
                    Some(&'(') => {
                        chars.next();
                        if chars.peek() == Some(&'(') {
                            // Nested $(( )) — collect and recursively expand
                            chars.next();
                            let mut depth = 1u32;
                            let mut inner = String::new();
                            loop {
                                match chars.next() {
                                    Some('(') if chars.peek() == Some(&'(') => {
                                        chars.next();
                                        depth += 1;
                                        inner.push_str("((");
                                    }
                                    Some(')') if chars.peek() == Some(&')') => {
                                        chars.next();
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                        inner.push_str("))");
                                    }
                                    Some(ch) => inner.push(ch),
                                    None => break,
                                }
                            }
                            let expanded = expand_arith_expr(os.clone(), proc, &inner).await;
                            let val = eval_arith(proc, &expanded);
                            result.push_str(&val.to_string());
                        } else {
                            // $(...) command substitution
                            let mut depth = 1u32;
                            let mut cmd = String::new();
                            loop {
                                match chars.next() {
                                    Some('(') => {
                                        depth += 1;
                                        cmd.push('(');
                                    }
                                    Some(')') => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                        cmd.push(')');
                                    }
                                    Some(ch) => cmd.push(ch),
                                    None => break,
                                }
                            }
                            let output = capture_output(os.clone(), proc, &cmd).await;
                            result.push_str(output.trim());
                        }
                    }
                    Some(&'{') => {
                        chars.next();
                        let mut name = String::new();
                        for ch in chars.by_ref() {
                            if ch == '}' {
                                break;
                            }
                            name.push(ch);
                        }
                        if let Some(val) = resolve_var(proc, &name) {
                            result.push_str(&val);
                        }
                    }
                    Some(&c2)
                        if c2.is_ascii_digit()
                            || c2 == '?'
                            || c2 == '#'
                            || c2 == '$'
                            || c2 == '!'
                            || c2 == '@'
                            || c2 == '*'
                            || c2 == '-' =>
                    {
                        chars.next();
                        let name = String::from(c2);
                        if let Some(val) = resolve_var(proc, &name) {
                            result.push_str(&val);
                        }
                    }
                    Some(&c2) if c2.is_ascii_alphabetic() || c2 == '_' => {
                        let mut name = String::new();
                        while let Some(&ch) = chars.peek() {
                            if ch.is_ascii_alphanumeric() || ch == '_' {
                                name.push(ch);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        if let Some(val) = resolve_var(proc, &name) {
                            result.push_str(&val);
                        }
                    }
                    _ => {
                        result.push('$');
                    }
                }
            } else {
                result.push(c);
                chars.next();
            }
        }
        result
    })
}

/// Arithmetic expression evaluator for $((expr)).
/// Supports: + - * / % ** parentheses, comparison, bitwise, logical, ternary,
/// assignment operators, comma. Bare variable names resolve to their integer value.
fn eval_arith(proc: &mut Process, expr: &str) -> i64 {
    let mut p = ArithParser::new(expr, proc);

    p.comma()
}

struct ArithParser<'a> {
    chars: Vec<char>,
    pos: usize,
    proc: &'a mut Process,
}

impl<'a> ArithParser<'a> {
    fn new(expr: &str, proc: &'a mut Process) -> Self {
        Self {
            chars: expr.chars().collect(),
            pos: 0,
            proc,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, ch: char) -> bool {
        self.skip_ws();
        if self.chars.get(self.pos) == Some(&ch) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat2(&mut self, a: char, b: char) -> bool {
        self.skip_ws();
        if self.chars.get(self.pos) == Some(&a) && self.chars.get(self.pos + 1) == Some(&b) {
            self.pos += 2;
            true
        } else {
            false
        }
    }

    /// Resolve a variable name to its integer value.
    fn var_val(&self, name: &str) -> i64 {
        self.proc
            .env
            .get(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    // Precedence levels (lowest to highest):
    // comma, assign, ternary, logor, logand, bitor, bitxor, bitand,
    // equality, relational, shift, additive, multiplicative, exponent, unary, primary

    fn comma(&mut self) -> i64 {
        let mut val = self.assign();
        while self.eat(',') {
            val = self.assign();
        }
        val
    }

    fn assign(&mut self) -> i64 {
        // Look ahead for var = expr, var += expr, etc.
        let save = self.pos;
        self.skip_ws();
        if self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphabetic() || self.chars[self.pos] == '_')
        {
            let start = self.pos;
            while self.pos < self.chars.len()
                && (self.chars[self.pos].is_ascii_alphanumeric() || self.chars[self.pos] == '_')
            {
                self.pos += 1;
            }
            let name: String = self.chars[start..self.pos].iter().collect();
            self.skip_ws();

            // Check for assignment operators
            if let Some(op) = self.try_assign_op() {
                let rhs = self.assign();
                let val = match op {
                    '=' => rhs,
                    '+' => self.var_val(&name) + rhs,
                    '-' => self.var_val(&name) - rhs,
                    '*' => self.var_val(&name) * rhs,
                    '/' => {
                        if rhs != 0 {
                            self.var_val(&name) / rhs
                        } else {
                            0
                        }
                    }
                    '%' => {
                        if rhs != 0 {
                            self.var_val(&name) % rhs
                        } else {
                            0
                        }
                    }
                    '&' => self.var_val(&name) & rhs,
                    '^' => self.var_val(&name) ^ rhs,
                    '|' => self.var_val(&name) | rhs,
                    'L' => self.var_val(&name) << rhs, // <<= encoded as 'L'
                    'R' => self.var_val(&name) >> rhs, // >>= encoded as 'R'
                    _ => rhs,
                };
                self.proc.set_env(&name, val.to_string());
                return val;
            }
            // Not an assignment — backtrack
            self.pos = save;
        } else {
            self.pos = save;
        }
        self.ternary()
    }

    /// Try to consume an assignment operator. Returns the op char or None.
    fn try_assign_op(&mut self) -> Option<char> {
        self.skip_ws();
        let c = self.chars.get(self.pos).copied()?;
        match c {
            '=' if self.chars.get(self.pos + 1) != Some(&'=') => {
                self.pos += 1;
                Some('=')
            }
            '+' | '-' | '*' | '/' | '%' | '&' | '^' | '|'
                if self.chars.get(self.pos + 1) == Some(&'=') =>
            {
                self.pos += 2;
                Some(c)
            }
            '<' if self.chars.get(self.pos + 1) == Some(&'<')
                && self.chars.get(self.pos + 2) == Some(&'=') =>
            {
                self.pos += 3;
                Some('L')
            }
            '>' if self.chars.get(self.pos + 1) == Some(&'>')
                && self.chars.get(self.pos + 2) == Some(&'=') =>
            {
                self.pos += 3;
                Some('R')
            }
            _ => None,
        }
    }

    fn ternary(&mut self) -> i64 {
        let cond = self.logor();
        if self.eat('?') {
            let then_val = self.assign();
            let _ = self.eat(':');
            let else_val = self.assign();
            if cond != 0 { then_val } else { else_val }
        } else {
            cond
        }
    }

    fn logor(&mut self) -> i64 {
        let mut val = self.logand();
        while self.eat2('|', '|') {
            val = if val != 0 || self.logand() != 0 { 1 } else { 0 };
        }
        val
    }

    fn logand(&mut self) -> i64 {
        let mut val = self.bitor();
        while self.eat2('&', '&') {
            val = if val != 0 && self.bitor() != 0 { 1 } else { 0 };
        }
        val
    }

    fn bitor(&mut self) -> i64 {
        let mut val = self.bitxor();
        loop {
            self.skip_ws();
            if self.chars.get(self.pos) == Some(&'|') && self.chars.get(self.pos + 1) != Some(&'|')
            {
                self.pos += 1;
                val |= self.bitxor();
            } else {
                break;
            }
        }
        val
    }

    fn bitxor(&mut self) -> i64 {
        let mut val = self.bitand();
        loop {
            self.skip_ws();
            if self.chars.get(self.pos) == Some(&'^') && self.chars.get(self.pos + 1) != Some(&'=')
            {
                self.pos += 1;
                val ^= self.bitand();
            } else {
                break;
            }
        }
        val
    }

    fn bitand(&mut self) -> i64 {
        let mut val = self.equality();
        loop {
            self.skip_ws();
            if self.chars.get(self.pos) == Some(&'&')
                && self.chars.get(self.pos + 1) != Some(&'&')
                && self.chars.get(self.pos + 1) != Some(&'=')
            {
                self.pos += 1;
                val &= self.equality();
            } else {
                break;
            }
        }
        val
    }

    fn equality(&mut self) -> i64 {
        let mut val = self.relational();
        loop {
            if self.eat2('=', '=') {
                val = (val == self.relational()) as i64;
            } else if self.eat2('!', '=') {
                val = (val != self.relational()) as i64;
            } else {
                break;
            }
        }
        val
    }

    fn relational(&mut self) -> i64 {
        let mut val = self.shift();
        loop {
            if self.eat2('<', '=') {
                val = (val <= self.shift()) as i64;
            } else if self.eat2('>', '=') {
                val = (val >= self.shift()) as i64;
            } else {
                self.skip_ws();
                let (c0, c1) = (
                    self.chars.get(self.pos).copied(),
                    self.chars.get(self.pos + 1).copied(),
                );
                if c0 == Some('<') && c1 != Some('<') && c1 != Some('=') {
                    self.pos += 1;
                    val = (val < self.shift()) as i64;
                } else if c0 == Some('>') && c1 != Some('>') && c1 != Some('=') {
                    self.pos += 1;
                    val = (val > self.shift()) as i64;
                } else {
                    break;
                }
            }
        }
        val
    }

    fn shift(&mut self) -> i64 {
        let mut val = self.additive();
        loop {
            self.skip_ws();
            if self.chars.get(self.pos) == Some(&'<')
                && self.chars.get(self.pos + 1) == Some(&'<')
                && self.chars.get(self.pos + 2) != Some(&'=')
            {
                self.pos += 2;
                val <<= self.additive();
            } else if self.chars.get(self.pos) == Some(&'>')
                && self.chars.get(self.pos + 1) == Some(&'>')
                && self.chars.get(self.pos + 2) != Some(&'=')
            {
                self.pos += 2;
                val >>= self.additive();
            } else {
                break;
            }
        }
        val
    }

    fn additive(&mut self) -> i64 {
        let mut val = self.multiplicative();
        loop {
            self.skip_ws();
            let c = self.chars.get(self.pos).copied();
            let c1 = self.chars.get(self.pos + 1).copied();
            if c == Some('+') && c1 != Some('=') && c1 != Some('+') {
                self.pos += 1;
                val += self.multiplicative();
            } else if c == Some('-') && c1 != Some('=') && c1 != Some('-') {
                self.pos += 1;
                val -= self.multiplicative();
            } else {
                break;
            }
        }
        val
    }

    fn multiplicative(&mut self) -> i64 {
        let mut val = self.exponent();
        loop {
            self.skip_ws();
            let c = self.chars.get(self.pos).copied();
            let c1 = self.chars.get(self.pos + 1).copied();
            if c == Some('*') && c1 != Some('*') && c1 != Some('=') {
                self.pos += 1;
                val *= self.exponent();
            } else if c == Some('/') && c1 != Some('=') {
                self.pos += 1;
                let r = self.exponent();
                val = if r != 0 { val / r } else { 0 };
            } else if c == Some('%') && c1 != Some('=') {
                self.pos += 1;
                let r = self.exponent();
                val = if r != 0 { val % r } else { 0 };
            } else {
                break;
            }
        }
        val
    }

    fn exponent(&mut self) -> i64 {
        let val = self.unary();
        if self.eat2('*', '*') {
            let exp = self.exponent(); // right-associative
            if exp < 0 {
                0
            } else {
                val.wrapping_pow(exp as u32)
            }
        } else {
            val
        }
    }

    fn unary(&mut self) -> i64 {
        self.skip_ws();
        match self.peek() {
            Some('-') if self.chars.get(self.pos + 1) != Some(&'=') => {
                self.pos += 1;
                -self.unary()
            }
            Some('+')
                if self.chars.get(self.pos + 1) != Some(&'=')
                    && self.chars.get(self.pos + 1) != Some(&'+') =>
            {
                self.pos += 1;
                self.unary()
            }
            Some('!') if self.chars.get(self.pos + 1) != Some(&'=') => {
                self.pos += 1;
                if self.unary() == 0 { 1 } else { 0 }
            }
            Some('~') => {
                self.pos += 1;
                !self.unary()
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> i64 {
        // Check for pre-increment/decrement
        if self.eat2('+', '+') {
            let name = self.read_name();
            let val = self.var_val(&name) + 1;
            self.proc.set_env(&name, val.to_string());
            return val;
        }
        if self.eat2('-', '-') {
            let name = self.read_name();
            let val = self.var_val(&name) - 1;
            self.proc.set_env(&name, val.to_string());
            return val;
        }

        // Post-increment/decrement: only if primary was a variable
        // We handle this by checking for ++ or -- after primary
        // but we need the variable name. For simplicity, check the chars.
        self.primary()
    }

    fn read_name(&mut self) -> String {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphanumeric() || self.chars[self.pos] == '_')
        {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn primary(&mut self) -> i64 {
        self.skip_ws();
        if self.pos >= self.chars.len() {
            return 0;
        }

        let ch = self.chars[self.pos];

        // Parenthesized expression
        if ch == '(' {
            self.pos += 1;
            let val = self.comma();
            let _ = self.eat(')');
            return val;
        }

        // Number (decimal, octal, hex)
        if ch.is_ascii_digit() {
            return self.read_number();
        }

        // $VAR reference
        if ch == '$' {
            self.pos += 1;
            if self.pos < self.chars.len() && self.chars[self.pos] == '{' {
                self.pos += 1;
                let name = self.read_name();
                let _ = self.eat('}');
                return self.var_val(&name);
            }
            let name = self.read_name();
            return self.var_val(&name);
        }

        // Bare variable name
        if ch.is_ascii_alphabetic() || ch == '_' {
            let name = self.read_name();
            return self.var_val(&name);
        }

        0
    }

    fn read_number(&mut self) -> i64 {
        let start = self.pos;
        if self.chars[self.pos] == '0' && self.pos + 1 < self.chars.len() {
            match self.chars[self.pos + 1] {
                'x' | 'X' => {
                    self.pos += 2;
                    while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_hexdigit() {
                        self.pos += 1;
                    }
                    let s: String = self.chars[start..self.pos].iter().collect();
                    return i64::from_str_radix(&s[2..], 16).unwrap_or(0);
                }
                '0'..='7' => {
                    self.pos += 1;
                    while self.pos < self.chars.len() && matches!(self.chars[self.pos], '0'..='7') {
                        self.pos += 1;
                    }
                    let s: String = self.chars[start + 1..self.pos].iter().collect();
                    return i64::from_str_radix(&s, 8).unwrap_or(0);
                }
                _ => {}
            }
        }
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse().unwrap_or(0)
    }
}

/// Resolve a variable name to its value (None if unset).
fn resolve_var(proc: &Process, name: &str) -> Option<String> {
    match name {
        "?" => Some(proc.last_exit.to_string()),
        "$" => Some(proc.pid.to_string()),
        "!" => proc.last_bg_pid.map(|p| p.to_string()),
        "#" => Some(proc.args.len().to_string()),
        "-" => {
            let mut flags = String::new();
            if proc.opt_errexit {
                flags.push('e');
            }
            if proc.opt_nounset {
                flags.push('u');
            }
            if proc.opt_xtrace {
                flags.push('x');
            }
            Some(flags)
        }
        "0" => Some(proc.arg0.clone()),
        "@" | "*" => Some(proc.args.join(" ")),
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_digit() => {
            let idx = (n.as_bytes()[0] - b'0') as usize;
            if idx > 0 {
                proc.args.get(idx - 1).cloned()
            } else {
                None
            }
        }
        _ => proc.env.get(name).cloned(),
    }
}

/// Check nounset: if `set -u` is active and the variable is unset, print error and set flag.
/// Returns true if the expansion should be suppressed (nounset error occurred).
fn check_nounset(proc: &mut Process, name: &str) -> bool {
    if !proc.opt_nounset {
        return false;
    }
    // Special variables never trigger nounset
    match name {
        "?" | "$" | "!" | "#" | "-" | "0" | "@" | "*" => return false,
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_digit() => return false,
        _ => {}
    }
    if proc.env.get(name).is_none() {
        proc.err_msg(&format!("strands-shell: {name}: parameter not set"));
        proc.nounset_error = true;
        return true;
    }
    false
}

fn expand_part<'a>(
    os: Arc<dyn Kernel>,
    proc: &'a mut Process,
    part: &'a WordPart,
    out: &'a mut String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    Box::pin(async move {
        match part {
            WordPart::Literal(s) | WordPart::SingleQuoted(s) => out.push_str(s),
            WordPart::Var(name) => {
                if check_nounset(proc, name) {
                    return;
                }
                if let Some(val) = resolve_var(proc, name) {
                    out.push_str(&val);
                }
            }
            WordPart::VarOp(name, op, word, colon) => {
                let val = resolve_var(proc, name);
                let is_unset_or_null = match &val {
                    None => true,
                    Some(v) => *colon && v.is_empty(),
                };
                match op.as_str() {
                    "len" => {
                        let len = val.as_deref().unwrap_or("").len();
                        out.push_str(&len.to_string());
                    }
                    "-" => {
                        if is_unset_or_null {
                            let w = expand_word(os, proc, word).await;
                            out.push_str(&w);
                        } else {
                            out.push_str(val.as_deref().unwrap_or(""));
                        }
                    }
                    "=" => {
                        if is_unset_or_null {
                            let w = expand_word(os, proc, word).await;
                            proc.set_env(name, &w);
                            out.push_str(&w);
                        } else {
                            out.push_str(val.as_deref().unwrap_or(""));
                        }
                    }
                    "?" => {
                        if is_unset_or_null {
                            let msg = if word.is_empty() {
                                format!("{name}: parameter not set")
                            } else {
                                expand_word(os, proc, word).await
                            };
                            proc.err_msg(&format!("strands-shell: {msg}"));
                            proc.nounset_error = true;
                        } else {
                            out.push_str(val.as_deref().unwrap_or(""));
                        }
                    }
                    "+" => {
                        if !is_unset_or_null {
                            let w = expand_word(os, proc, word).await;
                            out.push_str(&w);
                        }
                    }
                    "%" | "%%" | "#" | "##" => {
                        let s = val.as_deref().unwrap_or("");
                        let pat = expand_word(os, proc, word).await;
                        out.push_str(&trim_pattern(s, &pat, op));
                    }
                    _ => {
                        if let Some(v) = &val {
                            out.push_str(v);
                        }
                    }
                }
            }
            WordPart::Backtick(cmd) | WordPart::DollarParen(cmd) => {
                let output = capture_output(os, proc, cmd).await;
                out.push_str(output.trim_end_matches('\n'));
            }
            WordPart::Arith(expr) => {
                let expanded = expand_arith_expr(os, proc, expr).await;
                let val = eval_arith(proc, &expanded);
                out.push_str(&val.to_string());
            }
            WordPart::Tilde(user) => {
                if user.is_empty() {
                    if let Some(home) = proc.env.get("HOME") {
                        out.push_str(home);
                    } else {
                        out.push('~');
                    }
                } else if user == "+" {
                    if let Some(pwd) = proc.env.get("PWD") {
                        out.push_str(pwd);
                    } else {
                        out.push('~');
                        out.push_str(user);
                    }
                } else if user == "-" {
                    if let Some(oldpwd) = proc.env.get("OLDPWD") {
                        out.push_str(oldpwd);
                    } else {
                        out.push('~');
                        out.push_str(user);
                    }
                } else {
                    // ~user: disabled for security — do not resolve
                    // system user home directories.
                    out.push('~');
                    out.push_str(user);
                }
            }
            WordPart::DoubleQuoted(parts) => {
                for p in parts {
                    expand_part(os.clone(), proc, p, out).await;
                }
            }
        }
    })
}

/// Pattern trimming for ${var%pat}, ${var%%pat}, ${var#pat}, ${var##pat}.
fn trim_pattern(s: &str, pat: &str, op: &str) -> String {
    let pat_bytes = pat.as_bytes();
    let s_bytes = s.as_bytes();
    match op {
        "%" => {
            // Remove shortest suffix matching pat
            for i in (0..=s_bytes.len()).rev() {
                if glob_match(pat_bytes, &s_bytes[i..]) {
                    return s[..i].to_string();
                }
            }
            s.to_string()
        }
        "%%" => {
            // Remove longest suffix matching pat
            for i in 0..=s_bytes.len() {
                if glob_match(pat_bytes, &s_bytes[i..]) {
                    return s[..i].to_string();
                }
            }
            s.to_string()
        }
        "#" => {
            // Remove shortest prefix matching pat
            for i in 0..=s_bytes.len() {
                if glob_match(pat_bytes, &s_bytes[..i]) {
                    return s[i..].to_string();
                }
            }
            s.to_string()
        }
        "##" => {
            // Remove longest prefix matching pat
            for i in (0..=s_bytes.len()).rev() {
                if glob_match(pat_bytes, &s_bytes[..i]) {
                    return s[i..].to_string();
                }
            }
            s.to_string()
        }
        _ => s.to_string(),
    }
}

/// Expand a here-doc body (like double-quoted context: expand $VAR, $(cmd), `cmd`).
async fn expand_heredoc_body(os: Arc<dyn Kernel>, proc: &mut Process, body: &str) -> String {
    // Parse the body as if it were inside double quotes
    let mut parts: Vec<WordPart> = Vec::new();
    let mut lit = String::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '$' => {
                if let Ok(Some(part)) = parser::collect_dollar_pub(&mut chars) {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    parts.push(part);
                } else {
                    lit.push('$');
                }
            }
            '`' => {
                if !lit.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                }
                let mut cmd = String::new();
                for c in chars.by_ref() {
                    if c == '`' {
                        break;
                    }
                    cmd.push(c);
                }
                parts.push(WordPart::Backtick(cmd));
            }
            '\\' => {
                if let Some(&next) = chars.peek() {
                    if "$`\\".contains(next) {
                        lit.push(next);
                        chars.next();
                    } else {
                        lit.push('\\');
                    }
                } else {
                    lit.push('\\');
                }
            }
            _ => lit.push(ch),
        }
    }
    if !lit.is_empty() {
        parts.push(WordPart::Literal(lit));
    }
    expand_word(os, proc, &parts).await
}

/// Apply expanded redirections to a process. Returns Ok(()) or an error message.
async fn apply_redirects(
    os: &Arc<dyn Kernel>,
    proc: &mut Process,
    redirects: &[(Redirect, String)],
) -> Result<(), String> {
    for (redir, target) in redirects {
        match redir {
            Redirect::Write(fd, _) => {
                let opened = os
                    .open(proc, target, OpenFlags::write())
                    .await
                    .map_err(|e| format!("{target}: {e}"))?;
                let _ = proc.dup2(opened, *fd);
            }
            Redirect::Append(fd, _) => {
                let opened = os
                    .open(proc, target, OpenFlags::append())
                    .await
                    .map_err(|e| format!("{target}: {e}"))?;
                let _ = proc.dup2(opened, *fd);
            }
            Redirect::Read(fd, _) => {
                let opened = os
                    .open(proc, target, OpenFlags::read())
                    .await
                    .map_err(|e| format!("{target}: {e}"))?;
                let _ = proc.dup2(opened, *fd);
            }
            Redirect::ReadWrite(fd, _) => {
                let opened = os
                    .open(
                        proc,
                        target,
                        OpenFlags {
                            read: true,
                            write: true,
                            create: true,
                            append: false,
                            truncate: false,
                        },
                    )
                    .await
                    .map_err(|e| format!("{target}: {e}"))?;
                let _ = proc.dup2(opened, *fd);
            }
            Redirect::Clobber(fd, _) => {
                let opened = os
                    .open(proc, target, OpenFlags::write())
                    .await
                    .map_err(|e| format!("{target}: {e}"))?;
                let _ = proc.dup2(opened, *fd);
            }
            Redirect::DupWrite(fd, _) | Redirect::DupRead(fd, _) => {
                if target == "-" {
                    proc.close(*fd);
                } else {
                    let src_fd: u32 = target
                        .parse()
                        .map_err(|_| format!("{target}: bad file descriptor"))?;
                    proc.dup_fd(src_fd, *fd)
                        .await
                        .map_err(|e| format!("{target}: {e}"))?;
                }
            }
            Redirect::HereDoc(fd, _, body, _, quoted) => {
                let content = if *quoted {
                    body.clone()
                } else {
                    expand_heredoc_body(os.clone(), proc, body).await
                };
                let (tx, rx) = os::pipe(64);
                let data = bytes::Bytes::from(content);
                tokio::spawn(async move {
                    let _ = tx.send(data).await;
                });
                proc.set_channel_reader(*fd, rx);
            }
        }
    }
    Ok(())
}

/// Expand a Vec<Word> (args) into Vec<String>, with IFS field splitting.
async fn expand_words(os: Arc<dyn Kernel>, proc: &mut Process, words: &[Word]) -> Vec<String> {
    let mut result = Vec::with_capacity(words.len());
    for w in words {
        result.extend(expand_word_split(os.clone(), proc, w).await);
    }
    result
}

/// Run a command string in a subshell and capture its stdout.
async fn capture_output(os: Arc<dyn Kernel>, proc: &mut Process, cmd: &str) -> String {
    if proc.max_input > 0 && cmd.len() > proc.max_input {
        proc.captured_stderr
            .push_str("strands-shell: input too large\n");
        proc.err_msg("strands-shell: input too large");
        proc.last_exit = 1;
        return String::new();
    }
    let command_line = match parser::parse(cmd) {
        Ok(cl) => cl,
        Err(_) => return String::new(),
    };
    let mut sub = proc.fork();
    sub.depth += 1;
    if sub.check_limits().is_some() {
        proc.captured_stderr
            .push_str("strands-shell: maximum recursion depth exceeded\n");
        proc.err_msg("strands-shell: maximum recursion depth exceeded");
        proc.last_exit = 1;
        return String::new();
    }
    let result = run_capturing(os, &mut sub, &command_line).await;
    if !sub.captured_stderr.is_empty() {
        proc.captured_stderr.push_str(&sub.captured_stderr);
    }
    result
}

/// Execute a command line, capturing all stdout into a String instead of printing it.
async fn run_capturing(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    command_line: &parser::CommandLine,
) -> String {
    let mut output = String::new();
    let mut last_exit = 0;
    let mut skip = false;
    for (i, (item, _connector)) in command_line.iter().enumerate() {
        if i > 0 {
            match &command_line[i - 1].1 {
                Some(Connector::And) => skip = last_exit != 0,
                Some(Connector::Or) => skip = last_exit == 0,
                _ => skip = false,
            }
        }
        if !skip {
            let (exit, out) = match item {
                Item::Pipeline(pipeline, negated) => {
                    let (mut exit, out) =
                        execute_pipeline_capture(os.clone(), proc, pipeline).await;
                    if *negated {
                        exit = if exit == 0 { 1 } else { 0 };
                    }
                    (exit, out)
                }
                Item::Group(cl) => {
                    let s = Box::pin(run_capturing(os.clone(), proc, cl)).await;
                    (0, s)
                }
                Item::Subshell(cl) => {
                    let mut sub = proc.fork();
                    sub.depth += 1;
                    if let Some(msg) = sub.check_limits() {
                        sub.err_msg(msg);
                        (1, String::new())
                    } else {
                        let s = Box::pin(run_capturing(os.clone(), &mut sub, cl)).await;
                        (0, s)
                    }
                }
                Item::If {
                    branches,
                    else_body,
                } => {
                    let mut result = (0, String::new());
                    let mut matched = false;
                    for (cond, body) in branches {
                        let exit = Box::pin(execute_command_line(os.clone(), proc, cond)).await;
                        if exit == 0 {
                            let s = Box::pin(run_capturing(os.clone(), proc, body)).await;
                            result = (0, s);
                            matched = true;
                            break;
                        }
                    }
                    if !matched && let Some(body) = else_body {
                        let s = Box::pin(run_capturing(os.clone(), proc, body)).await;
                        result = (0, s);
                    }
                    result
                }
                Item::While { condition, body } | Item::Until { condition, body } => {
                    let is_until = matches!(item, Item::Until { .. });
                    let mut s = String::new();
                    loop {
                        let cond_exit =
                            Box::pin(execute_command_line(os.clone(), proc, condition)).await;
                        if (is_until && cond_exit == 0) || (!is_until && cond_exit != 0) {
                            break;
                        }
                        s.push_str(&Box::pin(run_capturing(os.clone(), proc, body)).await);
                    }
                    (0, s)
                }
                Item::For { var, words, body } => {
                    let expanded = expand_words(os.clone(), proc, words).await;
                    let mut s = String::new();
                    for val in &expanded {
                        proc.set_env(var, val);
                        s.push_str(&Box::pin(run_capturing(os.clone(), proc, body)).await);
                    }
                    (0, s)
                }
                Item::Case { word, arms } => {
                    let value = expand_word(os.clone(), proc, word).await;
                    let mut s = String::new();
                    for arm in arms {
                        let mut matched = false;
                        for pat in &arm.patterns {
                            let p = expand_word(os.clone(), proc, pat).await;
                            if case_match(&p, &value) {
                                matched = true;
                                break;
                            }
                        }
                        if matched {
                            s = Box::pin(run_capturing(os.clone(), proc, &arm.body)).await;
                            break;
                        }
                    }
                    (0, s)
                }
                Item::CompoundPipeline {
                    compound,
                    tail,
                    negated,
                } => {
                    let compound_cl = vec![(*compound.clone(), None)];
                    let captured = Box::pin(run_capturing(os.clone(), proc, &compound_cl)).await;
                    let (tx, rx) = os::pipe(64);
                    let handle = tokio::task::spawn_local(async move {
                        use bytes::Bytes;
                        if !captured.is_empty() {
                            let _ = tx.send(Bytes::from(captured.into_bytes())).await;
                        }
                    });
                    proc.set_channel_reader(0, rx);
                    let (mut exit, out) = execute_pipeline_capture(os.clone(), proc, tail).await;
                    let _ = handle.await;
                    proc.close(0);
                    if *negated {
                        exit = if exit == 0 { 1 } else { 0 };
                    }
                    (exit, out)
                }
                Item::Function { name, body } => {
                    proc.set_function(name, body.clone());
                    (0, String::new())
                }
                Item::CompoundRedirect { item, redirects } => {
                    let mut expanded = Vec::new();
                    for redir in redirects {
                        let target = match redir {
                            Redirect::Write(_, w)
                            | Redirect::Append(_, w)
                            | Redirect::Read(_, w)
                            | Redirect::ReadWrite(_, w)
                            | Redirect::Clobber(_, w)
                            | Redirect::DupWrite(_, w)
                            | Redirect::DupRead(_, w) => expand_word(os.clone(), proc, w).await,
                            Redirect::HereDoc(..) => String::new(),
                        };
                        expanded.push((redir.clone(), target));
                    }
                    // Separate stdin vs stdout redirects
                    let mut stdin_redir = Vec::new();
                    let mut stdout_redir = Vec::new();
                    for (redir, target) in &expanded {
                        match redir {
                            Redirect::Read(..)
                            | Redirect::ReadWrite(..)
                            | Redirect::DupRead(..)
                            | Redirect::HereDoc(..) => {
                                stdin_redir.push((redir.clone(), target.clone()))
                            }
                            _ => stdout_redir.push((redir.clone(), target.clone())),
                        }
                    }
                    let mut sub = proc.fork();
                    if !stdin_redir.is_empty() {
                        if let Err(msg) = apply_redirects(&os, &mut sub, &stdin_redir).await {
                            proc.err_msg(&format!("strands-shell: {msg}"));
                            (1, String::new())
                        } else {
                            let inner_cl = vec![(*item.clone(), None)];
                            let s = Box::pin(run_capturing(os.clone(), &mut sub, &inner_cl)).await;
                            (0, s)
                        }
                    } else if !stdout_redir.is_empty() {
                        // Use execute_item which handles CompoundRedirect properly
                        let cr = Item::CompoundRedirect {
                            item: item.clone(),
                            redirects: redirects.clone(),
                        };
                        let exit = Box::pin(execute_item(os.clone(), proc, &cr))
                            .await
                            .unwrap_or(1);
                        (exit, String::new())
                    } else {
                        let inner_cl = vec![(*item.clone(), None)];
                        let s = Box::pin(run_capturing(os.clone(), &mut sub, &inner_cl)).await;
                        (0, s)
                    }
                }
            };
            last_exit = exit;
            output.push_str(&out);
            if proc.max_output > 0 && output.len() > proc.max_output {
                proc.err_msg("strands-shell: output size limit exceeded");
                output.truncate(proc.max_output);
                proc.last_exit = 1;
                break;
            }
        }
    }
    output
}

/// Public-facing execute_command_line that swallows break/continue.
async fn execute_command_line(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    command_line: &parser::CommandLine,
) -> i32 {
    match execute_command_line_inner(os, proc, command_line).await {
        Ok(code) => code,
        Err(ControlFlow::Exit(code)) => code,
        Err(_) => proc.last_exit,
    }
}

/// Inner execute that propagates ControlFlow.
async fn execute_command_line_inner(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    command_line: &parser::CommandLine,
) -> Result<i32, ControlFlow> {
    let mut last_exit = 0i32;
    let mut skip = false;
    for (i, (item, connector)) in command_line.iter().enumerate() {
        if i > 0 {
            match &command_line[i - 1].1 {
                Some(Connector::And) => skip = last_exit != 0,
                Some(Connector::Or) => skip = last_exit == 0,
                Some(Connector::Background) => skip = false,
                _ => skip = false,
            }
        }
        if !skip {
            // Background: spawn in a forked process, don't wait
            if matches!(connector, Some(Connector::Background)) {
                if proc.max_bg_jobs > 0 && proc.bg_jobs.len() >= proc.max_bg_jobs {
                    proc.err_msg("strands-shell: too many background jobs");
                    last_exit = 1;
                    proc.last_exit = 1;
                    continue;
                }
                let os2 = os.clone();
                let mut sub = proc.fork();
                let item = item.clone();
                proc.bg_counter += 1;
                let pid = proc.bg_counter;
                proc.last_bg_pid = Some(pid);
                let handle = tokio::task::spawn_local(async move {
                    let code = match execute_item(os2, &mut sub, &item).await {
                        Ok(n) => n,
                        Err(ControlFlow::Exit(n)) => n,
                        Err(_) => sub.last_exit,
                    };
                    let stdout = std::mem::take(&mut sub.captured_output);
                    let stderr = std::mem::take(&mut sub.captured_stderr);
                    (code, stdout, stderr)
                });
                proc.bg_jobs.push(handle);
                last_exit = 0;
                proc.last_exit = 0;
                continue;
            }

            // Determine if this item is in a "tested" context (suppresses errexit).
            // Tested = followed by && or ||.
            let tested = matches!(connector, Some(Connector::And) | Some(Connector::Or));

            last_exit = execute_item(os.clone(), proc, item).await?;
            proc.last_exit = last_exit;

            // errexit: if set -e is active and command failed and not in tested context
            if proc.opt_errexit && last_exit != 0 && !tested {
                return Err(ControlFlow::Exit(last_exit));
            }
        }
    }
    Ok(last_exit)
}

async fn execute_item(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    item: &parser::Item,
) -> Result<i32, ControlFlow> {
    if let Some(msg) = proc.check_limits() {
        proc.err_msg(msg);
        return Err(ControlFlow::Exit(1));
    }
    match item {
        Item::Pipeline(pipeline, negated) => {
            let mut exit = execute_pipeline_checked(os, proc, pipeline).await?;
            if *negated {
                exit = if exit == 0 { 1 } else { 0 };
            }
            Ok(exit)
        }
        Item::Group(cl) => Box::pin(execute_command_line_inner(os, proc, cl)).await,
        Item::Subshell(cl) => {
            let mut sub = proc.fork();
            sub.depth += 1;
            if let Some(msg) = sub.check_limits() {
                sub.err_msg(msg);
                return Ok(1);
            }
            let r = Box::pin(execute_command_line_inner(os, &mut sub, cl)).await;
            if proc.capture {
                proc.captured_output.push_str(&sub.captured_output);
                proc.captured_stderr.push_str(&sub.captured_stderr);
            }
            match r {
                Ok(n) => Ok(n),
                Err(ControlFlow::Exit(n)) => Ok(n),
                Err(e) => Err(e),
            }
        }
        Item::If {
            branches,
            else_body,
        } => Box::pin(execute_if(os, proc, branches, else_body.as_ref())).await,
        Item::While { condition, body } => {
            Box::pin(execute_while(os, proc, condition, body, false)).await
        }
        Item::Until { condition, body } => {
            Box::pin(execute_while(os, proc, condition, body, true)).await
        }
        Item::For { var, words, body } => Box::pin(execute_for(os, proc, var, words, body)).await,
        Item::Case { word, arms } => Box::pin(execute_case(os, proc, word, arms)).await,
        Item::Function { name, body } => {
            proc.set_function(name, body.clone());
            Ok(0)
        }
        Item::CompoundPipeline {
            compound,
            tail,
            negated,
        } => {
            // Capture the compound command's output, then pipe it as
            // stdin into the tail pipeline.
            let compound_cl = vec![(*compound.clone(), None)];
            let mut sub = proc.fork();
            let os2 = os.clone();

            let (tx, rx) = os::pipe(64);
            let handle = tokio::task::spawn_local(async move {
                let output = Box::pin(run_capturing(os2, &mut sub, &compound_cl)).await;
                use bytes::Bytes;
                if !output.is_empty() {
                    let _ = tx.send(Bytes::from(output.into_bytes())).await;
                }
            });

            // Set stdin on proc so the tail pipeline inherits it.
            proc.set_channel_reader(0, rx);
            let mut exit = execute_pipeline(os, proc, tail).await;
            let _ = handle.await;
            proc.close(0);
            if *negated {
                exit = if exit == 0 { 1 } else { 0 };
            }
            Ok(exit)
        }
        Item::CompoundRedirect { item, redirects } => {
            let mut expanded = Vec::new();
            for redir in redirects {
                let target = match redir {
                    Redirect::Write(_, w)
                    | Redirect::Append(_, w)
                    | Redirect::Read(_, w)
                    | Redirect::ReadWrite(_, w)
                    | Redirect::Clobber(_, w)
                    | Redirect::DupWrite(_, w)
                    | Redirect::DupRead(_, w) => expand_word(os.clone(), proc, w).await,
                    Redirect::HereDoc(..) => String::new(),
                };
                expanded.push((redir.clone(), target));
            }
            // For stdin redirects, apply to proc so inner commands can read
            let mut stdin_expanded = Vec::new();
            let mut stdout_expanded = Vec::new();
            for (redir, target) in &expanded {
                match redir {
                    Redirect::Read(..)
                    | Redirect::ReadWrite(..)
                    | Redirect::DupRead(..)
                    | Redirect::HereDoc(..) => stdin_expanded.push((redir.clone(), target.clone())),
                    _ => stdout_expanded.push((redir.clone(), target.clone())),
                }
            }
            // Apply stdin redirects to proc
            let mut saved_stdin = Process::empty();
            if !stdin_expanded.is_empty() {
                proc.transfer_fd(STDIN, &mut saved_stdin);
                if let Err(msg) = apply_redirects(&os, proc, &stdin_expanded).await {
                    saved_stdin.transfer_fd(STDIN, proc);
                    proc.err_msg(&format!("strands-shell: {msg}"));
                    return Ok(1);
                }
            }
            // For stdout/stderr redirects, capture output and write to target
            if !stdout_expanded.is_empty() {
                let inner_cl = vec![(*item.clone(), None)];
                let captured = Box::pin(run_capturing(os.clone(), proc, &inner_cl)).await;
                if !stdin_expanded.is_empty() {
                    proc.close(STDIN);
                    saved_stdin.transfer_fd(STDIN, proc);
                }
                // Write captured output to each redirect target
                for (redir, target) in &stdout_expanded {
                    let fd_num = match redir {
                        Redirect::Write(fd, _)
                        | Redirect::Append(fd, _)
                        | Redirect::Clobber(fd, _)
                        | Redirect::DupWrite(fd, _) => *fd,
                        _ => continue,
                    };
                    if fd_num == STDOUT || fd_num == 1 {
                        let flags = match redir {
                            Redirect::Append(..) => OpenFlags::append(),
                            _ => OpenFlags::write(),
                        };
                        if let Ok(fd) = os.open(proc, target, flags).await
                            && let Ok(mut w) = proc.take_writer(fd)
                        {
                            use tokio::io::AsyncWriteExt;
                            let _ = w.write_all(captured.as_bytes()).await;
                        }
                    }
                }
                // Yield to let write-back tasks flush
                tokio::task::yield_now().await;
                Ok(proc.last_exit)
            } else {
                let result = Box::pin(execute_item(os, proc, item)).await;
                if !stdin_expanded.is_empty() {
                    proc.close(STDIN);
                    saved_stdin.transfer_fd(STDIN, proc);
                }
                result
            }
        }
    }
}

async fn execute_if(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    branches: &[(parser::CommandLine, parser::CommandLine)],
    else_body: Option<&parser::CommandLine>,
) -> Result<i32, ControlFlow> {
    for (cond, body) in branches {
        // Condition is a "tested" context — suppress errexit
        let saved = proc.opt_errexit;
        proc.opt_errexit = false;
        let exit = execute_command_line_inner(os.clone(), proc, cond).await?;
        proc.opt_errexit = saved;
        proc.last_exit = exit;
        if exit == 0 {
            return execute_command_line_inner(os.clone(), proc, body).await;
        }
    }
    if let Some(body) = else_body {
        return execute_command_line_inner(os.clone(), proc, body).await;
    }
    Ok(0)
}

async fn execute_while(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    condition: &parser::CommandLine,
    body: &parser::CommandLine,
    invert: bool, // true for `until`
) -> Result<i32, ControlFlow> {
    let mut last_exit = 0;
    loop {
        // Condition is a "tested" context — suppress errexit
        let saved = proc.opt_errexit;
        proc.opt_errexit = false;
        let cond_exit = execute_command_line_inner(os.clone(), proc, condition).await?;
        proc.opt_errexit = saved;
        proc.last_exit = cond_exit;
        let should_run = if invert {
            cond_exit != 0
        } else {
            cond_exit == 0
        };
        if !should_run {
            break;
        }
        match execute_command_line_inner(os.clone(), proc, body).await {
            Ok(exit) => last_exit = exit,
            Err(ControlFlow::Break(n)) => {
                if n > 1 {
                    return Err(ControlFlow::Break(n - 1));
                }
                break;
            }
            Err(ControlFlow::Continue(n)) => {
                if n > 1 {
                    return Err(ControlFlow::Continue(n - 1));
                }
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(last_exit)
}

async fn execute_for(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    var: &str,
    words: &[Word],
    body: &parser::CommandLine,
) -> Result<i32, ControlFlow> {
    let expanded = expand_words(os.clone(), proc, words).await;
    let mut last_exit = 0;
    for val in &expanded {
        proc.set_env(var, val);
        match execute_command_line_inner(os.clone(), proc, body).await {
            Ok(exit) => last_exit = exit,
            Err(ControlFlow::Break(n)) => {
                if n > 1 {
                    return Err(ControlFlow::Break(n - 1));
                }
                break;
            }
            Err(ControlFlow::Continue(n)) => {
                if n > 1 {
                    return Err(ControlFlow::Continue(n - 1));
                }
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(last_exit)
}

async fn execute_case(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    word: &Word,
    arms: &[parser::CaseArm],
) -> Result<i32, ControlFlow> {
    let value = expand_word(os.clone(), proc, word).await;
    for arm in arms {
        let mut matched = false;
        for pat in &arm.patterns {
            let pattern = expand_word(os.clone(), proc, pat).await;
            if case_match(&pattern, &value) {
                matched = true;
                break;
            }
        }
        if matched {
            return execute_command_line_inner(os, proc, &arm.body).await;
        }
    }
    Ok(0)
}

/// Simple glob-style pattern matching for case statements.
fn case_match(pattern: &str, value: &str) -> bool {
    glob_match(pattern.as_bytes(), value.as_bytes())
}

fn glob_match(pat: &[u8], val: &[u8]) -> bool {
    let (mut pi, mut vi) = (0, 0);
    let (mut star_p, mut star_v) = (usize::MAX, 0);
    while vi < val.len() {
        if pi < pat.len() && pat[pi] == b'[' {
            // Character class
            if let Some((matched, end)) = glob_bracket(pat, pi, val[vi]) {
                if matched {
                    pi = end;
                    vi += 1;
                } else if star_p != usize::MAX {
                    pi = star_p + 1;
                    star_v += 1;
                    vi = star_v;
                } else {
                    return false;
                }
            } else {
                // Malformed bracket — treat '[' as literal
                if pat[pi] == val[vi] {
                    pi += 1;
                    vi += 1;
                } else if star_p != usize::MAX {
                    pi = star_p + 1;
                    star_v += 1;
                    vi = star_v;
                } else {
                    return false;
                }
            }
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_p = pi;
            star_v = vi;
            pi += 1;
        } else if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == val[vi]) {
            pi += 1;
            vi += 1;
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_v += 1;
            vi = star_v;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

/// Parse a bracket expression `[...]` starting at `pat[start]`.
/// Returns `Some((matched, end_index))` where `end_index` is past the `]`,
/// or `None` if the bracket is malformed (no closing `]`).
fn glob_bracket(pat: &[u8], start: usize, ch: u8) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < pat.len() && (pat[i] == b'!' || pat[i] == b'^');
    if negate {
        i += 1;
    }
    // First char after `[` (or `[!`) can be `]` as a literal
    let mut matched = false;
    let mut first = true;
    while i < pat.len() {
        if pat[i] == b']' && !first {
            return Some((matched ^ negate, i + 1));
        }
        first = false;
        // Range: a-z
        if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
            let lo = pat[i];
            let hi = pat[i + 2];
            if ch >= lo && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if pat[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    None // no closing ]
}

/// Print xtrace for a command about to execute.
fn xtrace(proc: &mut Process, args: &[String]) {
    if !proc.opt_xtrace {
        return;
    }
    let ps4 = proc.env.get("PS4").cloned().unwrap_or_default();
    let prefix = if ps4.is_empty() {
        "+ ".to_string()
    } else {
        ps4
    };
    proc.err_msg(&format!("{prefix}{}", args.join(" ")));
}

/// Execute a pipeline, checking for break/continue/exit builtins.
async fn execute_pipeline_checked(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    pipeline: &[parser::Command],
) -> Result<i32, ControlFlow> {
    // Check for break/continue/exit/eval/exec/./command before running
    if pipeline.len() == 1 && !pipeline[0].args.is_empty() {
        let name_word = &pipeline[0].args[0];
        if let Some(name) = parser::word_to_str(name_word) {
            match name.as_str() {
                "break" => {
                    let n = get_numeric_arg(os.clone(), proc, pipeline).await;
                    return Err(ControlFlow::Break(n));
                }
                "continue" => {
                    let n = get_numeric_arg(os.clone(), proc, pipeline).await;
                    return Err(ControlFlow::Continue(n));
                }
                "exit" => {
                    let n = if pipeline[0].args.len() > 1 {
                        let args = expand_words(os.clone(), proc, &pipeline[0].args[1..]).await;
                        args[0].parse().unwrap_or(1)
                    } else {
                        proc.last_exit
                    };
                    return Err(ControlFlow::Exit(n));
                }
                "return" => {
                    let n = if pipeline[0].args.len() > 1 {
                        let args = expand_words(os.clone(), proc, &pipeline[0].args[1..]).await;
                        args[0].parse().unwrap_or(1)
                    } else {
                        proc.last_exit
                    };
                    return Err(ControlFlow::Return(n));
                }
                "eval" => {
                    let args = expand_words(os.clone(), proc, &pipeline[0].args[1..]).await;
                    xtrace(proc, &[&["eval".into()], &args[..]].concat());
                    let code = args.join(" ");
                    if code.is_empty() {
                        return Ok(0);
                    }
                    proc.depth += 1;
                    let (exit, should_exit) = Box::pin(execute(os, proc, &code)).await;
                    proc.depth -= 1;
                    if should_exit {
                        return Err(ControlFlow::Exit(exit));
                    }
                    return Ok(exit);
                }
                "." | "source" => {
                    let args = expand_words(os.clone(), proc, &pipeline[0].args[1..]).await;
                    xtrace(proc, &[&[".".into()], &args[..]].concat());
                    if args.is_empty() {
                        proc.err_msg("strands-shell: .: filename argument required");
                        return Ok(2);
                    }
                    let fd = match os.open(proc, &args[0], OpenFlags::read()).await {
                        Ok(fd) => fd,
                        Err(e) => {
                            proc.err_msg(&format!("strands-shell: .: {}: {e}", args[0]));
                            return Ok(1);
                        }
                    };
                    let mut reader = proc.take_reader(fd).map_err(|e| {
                        proc.err_msg(&format!("strands-shell: .: {}: {e}", args[0]));
                        ControlFlow::Exit(1)
                    })?;
                    let content =
                        match os::read_to_string_limited(&mut reader, proc.max_output).await {
                            Ok(s) => s,
                            Err(e) => {
                                proc.err_msg(&format!("strands-shell: .: {}: {e}", args[0]));
                                return Ok(1);
                            }
                        };
                    let saved_args = if args.len() > 1 {
                        Some(std::mem::replace(&mut proc.args, args[1..].to_vec()))
                    } else {
                        None
                    };
                    proc.depth += 1;
                    let (exit, should_exit) =
                        Box::pin(execute_sourced(os.clone(), proc, &content)).await;
                    proc.depth -= 1;
                    if let Some(saved) = saved_args {
                        proc.args = saved;
                    }
                    if should_exit {
                        return Err(ControlFlow::Exit(exit));
                    }
                    return Ok(exit);
                }
                "exec" => {
                    let args = expand_words(os.clone(), proc, &pipeline[0].args[1..]).await;
                    if args.is_empty() {
                        // `exec` with no args — just apply redirects
                        // TODO: apply redirects to the shell process
                        return Ok(0);
                    }
                    // exec with args — try to run as a command
                    // For now, treat it like running the command directly
                    let fake_cmd = parser::Command {
                        env: pipeline[0].env.clone(),
                        args: pipeline[0].args[1..].to_vec(),
                        redirects: pipeline[0].redirects.clone(),
                    };
                    let exit = execute_pipeline(os, proc, &[fake_cmd]).await;
                    return Err(ControlFlow::Exit(exit));
                }
                "command" => {
                    if pipeline[0].args.len() < 2 {
                        return Ok(0);
                    }
                    let args = expand_words(os.clone(), proc, &pipeline[0].args[1..]).await;
                    if args.first().map(|s| s.as_str()) == Some("-v")
                        || args.first().map(|s| s.as_str()) == Some("-V")
                    {
                        let verbose = args[0] == "-V";
                        let mut status = 0;
                        for name in &args[1..] {
                            if is_special(name)
                                || crate::builtins::lookup(name).is_some()
                                || crate::commands::lookup(name).is_some()
                            {
                                if verbose {
                                    let label = if is_special(name) {
                                        "a special shell builtin"
                                    } else {
                                        "a shell builtin"
                                    };
                                    proc.out_msg(&format!("{name} is {label}"));
                                } else {
                                    proc.out_msg(name);
                                }
                            } else if proc.get_function(name).is_some() {
                                if verbose {
                                    proc.out_msg(&format!("{name} is a shell function"));
                                } else {
                                    proc.out_msg(name);
                                }
                            } else if let Some(path) = proc.hash_table.get(name.as_str()).cloned() {
                                proc.out_msg(&path);
                            } else if let Some(path) = find_in_path(&*os, proc, name).await {
                                proc.out_msg(&path);
                            } else {
                                status = 1;
                            }
                        }
                        return Ok(status);
                    }
                    // `command name args...` — run name bypassing functions
                    let fake_cmd = parser::Command {
                        env: pipeline[0].env.clone(),
                        args: pipeline[0].args[1..].to_vec(),
                        redirects: pipeline[0].redirects.clone(),
                    };
                    return Ok(execute_pipeline(os, proc, &[fake_cmd]).await);
                }
                _ => {}
            }
        }
    }
    let exit = execute_pipeline(os, proc, pipeline).await;
    // nounset errors are fatal — abort the shell
    if proc.nounset_error {
        proc.nounset_error = false;
        return Err(ControlFlow::Exit(exit));
    }
    Ok(exit)
}

/// Get the optional numeric argument for break/continue (default 1).
async fn get_numeric_arg(
    os: Arc<dyn Kernel>,
    proc: &mut Process,
    pipeline: &[parser::Command],
) -> i32 {
    if pipeline[0].args.len() > 1 {
        let args = expand_words(os, proc, &pipeline[0].args[1..]).await;
        args[0].parse().unwrap_or(1).max(1)
    } else {
        1
    }
}

async fn execute_pipeline(
    os: Arc<dyn Kernel>,
    shell_proc: &mut Process,
    pipeline: &[parser::Command],
) -> i32 {
    let capture = shell_proc.capture;
    let (exit, output) = run_pipeline(os, shell_proc, pipeline, capture).await;
    if capture {
        if shell_proc.max_output > 0
            && shell_proc.captured_output.len() + output.len() > shell_proc.max_output
        {
            let remaining = shell_proc
                .max_output
                .saturating_sub(shell_proc.captured_output.len());
            shell_proc.captured_output.push_str(&output[..remaining]);
            shell_proc.err_msg("strands-shell: output size limit exceeded");
            return 1;
        }
        shell_proc.captured_output.push_str(&output);
    }
    exit
}

async fn execute_pipeline_capture(
    os: Arc<dyn Kernel>,
    shell_proc: &mut Process,
    pipeline: &[parser::Command],
) -> (i32, String) {
    run_pipeline(os, shell_proc, pipeline, true).await
}

async fn run_pipeline(
    os: Arc<dyn Kernel>,
    shell_proc: &mut Process,
    pipeline: &[parser::Command],
    capture: bool,
) -> (i32, String) {
    let len = pipeline.len();

    if shell_proc.max_pipeline > 0 && len > shell_proc.max_pipeline {
        shell_proc.err_msg("strands-shell: pipeline too long");
        return (1, String::new());
    }

    // Expand all args/env/redirects up front
    let mut expanded_args: Vec<Vec<String>> = Vec::with_capacity(len);
    let mut expanded_env: Vec<Vec<(String, String)>> = Vec::with_capacity(len);
    let mut expanded_redirects: Vec<Vec<(Redirect, String)>> = Vec::with_capacity(len);

    for cmd in pipeline {
        expanded_args.push(expand_words(os.clone(), shell_proc, &cmd.args).await);
        let mut env = Vec::new();
        for (k, v) in &cmd.env {
            env.push((
                expand_word(os.clone(), shell_proc, k).await,
                expand_word(os.clone(), shell_proc, v).await,
            ));
        }
        expanded_env.push(env);
        let mut redirs = Vec::new();
        for r in &cmd.redirects {
            let (word, expanded) = match r {
                Redirect::Write(_, w)
                | Redirect::Append(_, w)
                | Redirect::Read(_, w)
                | Redirect::ReadWrite(_, w)
                | Redirect::Clobber(_, w)
                | Redirect::DupWrite(_, w)
                | Redirect::DupRead(_, w) => {
                    (r.clone(), expand_word(os.clone(), shell_proc, w).await)
                }
                Redirect::HereDoc(..) => (r.clone(), String::new()),
            };
            redirs.push((word, expanded));
        }
        expanded_redirects.push(redirs);
    }

    // Abort if nounset error occurred during expansion
    if shell_proc.nounset_error {
        shell_proc.last_exit = 2;
        return (2, String::new());
    }

    // xtrace: print expanded commands to stderr
    if shell_proc.opt_xtrace {
        for i in 0..len {
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in &expanded_env[i] {
                parts.push(format!("{k}={v}"));
            }
            parts.extend_from_slice(&expanded_args[i]);
            xtrace(shell_proc, &parts);
        }
    }

    // Bare assignment: no command, just VAR=value — apply to shell env
    if len == 1 && expanded_args[0].is_empty() {
        for (k, v) in &expanded_env[0] {
            shell_proc.set_env(k, v);
        }
        return (0, String::new());
    }

    // Resolve executables: for each stage, if the command is not a builtin,
    // function, or registered command, try to resolve it as an executable file.
    for i in 0..len {
        if expanded_args[i].is_empty() {
            continue;
        }
        let cmd = &expanded_args[i][0];
        if builtins::lookup(cmd).is_some()
            || shell_proc.get_function(cmd).is_some()
            || commands::lookup(cmd).is_some()
        {
            continue;
        }
        if let Some((_path, target)) = resolve_executable(os.as_ref(), shell_proc, cmd).await {
            match target {
                ExecTarget::Multicall(name) => {
                    expanded_args[i][0] = name;
                }
                ExecTarget::Shebang(interp) => {
                    // Check if the interpreter resolves to a multicall binary
                    if let Some((_, ExecTarget::Multicall(mcname))) =
                        resolve_executable(os.as_ref(), shell_proc, &interp[0]).await
                    {
                        // Multicall: rewrite to <command> <script_path> [orig_args...]
                        let script_path = interp.last().unwrap().clone();
                        let orig_args = expanded_args[i][1..].to_vec();
                        let mut new_args = vec![mcname, script_path];
                        new_args.extend(orig_args);
                        expanded_args[i] = new_args;
                    } else {
                        let orig_args = expanded_args[i][1..].to_vec();
                        let mut new_args = interp;
                        new_args.extend(orig_args);
                        expanded_args[i] = new_args;
                    }
                }
            }
        }
    }

    // Single command: check for builtins
    if len == 1 {
        if let Some(f) = builtins::lookup(&expanded_args[0][0]) {
            let name = expanded_args[0][0].clone();
            let args: Vec<String> = expanded_args[0][1..].to_vec();

            let mut io_proc = shell_proc.fork();
            // Inherit stdin from shell_proc if present (e.g. CompoundPipeline).
            shell_proc.transfer_fd(STDIN, &mut io_proc);
            let (out_tx, out_rx) = os::pipe(64);
            let (err_tx, err_rx) = os::pipe(64);
            io_proc.set_channel_writer(STDOUT, out_tx);
            io_proc.set_channel_writer(STDERR, err_tx.clone());
            shell_proc.set_err_tx(err_tx);

            // Apply redirects to the io_proc so builtins see them
            if let Err(e) = apply_redirects(&os, &mut io_proc, &expanded_redirects[0]).await {
                shell_proc.err_msg(&format!("strands-shell: {e}"));
                return (1, String::new());
            }

            // Drain stdout/stderr concurrently with the builtin to avoid
            // deadlock when the builtin produces more output than the channel
            // capacity.
            let max_output = shell_proc.max_output;
            enum Drain {
                Capture(tokio::task::JoinHandle<String>),
                Copy(tokio::task::JoinHandle<()>),
            }
            let stdout_drain = if capture {
                Drain::Capture(tokio::task::spawn_local(async move {
                    let mut reader = FdReader::from_receiver(out_rx);
                    os::read_to_string_limited(&mut reader, max_output)
                        .await
                        .unwrap_or_default()
                }))
            } else {
                Drain::Copy(tokio::task::spawn_local(async move {
                    let mut reader = FdReader::from_receiver(out_rx);
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let _ = tokio::io::copy(&mut reader, &mut tokio::io::stdout()).await;
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        use tokio::io::AsyncReadExt;
                        let mut buf = Vec::new();
                        let _ = reader.read_to_end(&mut buf).await;
                        let _ = std::io::Write::write_all(&mut std::io::stdout(), &buf);
                    }
                }))
            };
            let stderr_drain = if capture {
                Drain::Capture(tokio::task::spawn_local(async move {
                    let mut reader = FdReader::from_receiver(err_rx);
                    os::read_to_string_limited(&mut reader, max_output)
                        .await
                        .unwrap_or_default()
                }))
            } else {
                Drain::Copy(tokio::task::spawn_local(async move {
                    let mut reader = FdReader::from_receiver(err_rx);
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let _ = tokio::io::copy(&mut reader, &mut tokio::io::stderr()).await;
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        use tokio::io::AsyncReadExt;
                        let mut buf = Vec::new();
                        let _ = reader.read_to_end(&mut buf).await;
                        let _ = std::io::Write::write_all(&mut std::io::stderr(), &buf);
                    }
                }))
            };

            let io_cell = RefCell::new(io_proc);
            let result = CURRENT_KERNEL
                .scope(
                    os.clone(),
                    CURRENT_PROCESS.scope(io_cell, async {
                        let r = f(os.as_ref(), shell_proc, &args).await;
                        // Transfer stdin back so it survives across loop iterations
                        CURRENT_PROCESS.with(|p| p.borrow_mut().transfer_fd(STDIN, shell_proc));
                        r
                    }),
                )
                .await;
            shell_proc.clear_err_tx();

            let stdout_str = match stdout_drain {
                Drain::Capture(h) => h.await.unwrap_or_default(),
                Drain::Copy(h) => {
                    let _ = h.await;
                    String::new()
                }
            };
            let stderr_str = match stderr_drain {
                Drain::Capture(h) => h.await.unwrap_or_default(),
                Drain::Copy(h) => {
                    let _ = h.await;
                    String::new()
                }
            };
            if capture && !stderr_str.is_empty() {
                shell_proc.captured_stderr.push_str(&stderr_str);
            }

            return match result {
                Ok(code) => (code, stdout_str),
                Err(e) => {
                    shell_proc.err_msg(&format!("strands-shell: {name}: {e}"));
                    (1, stdout_str)
                }
            };
        }

        // Single command: check for functions
        if let Some(func_body) = shell_proc.get_function(&expanded_args[0][0]).cloned() {
            let saved_args =
                std::mem::replace(&mut shell_proc.args, expanded_args[0][1..].to_vec());
            shell_proc.push_local_scope();
            shell_proc.depth += 1;
            let exit = match Box::pin(execute_command_line_inner(
                os.clone(),
                shell_proc,
                &func_body,
            ))
            .await
            {
                Ok(code) => code,
                Err(ControlFlow::Return(code)) => code,
                Err(ControlFlow::Exit(code)) => {
                    shell_proc.depth -= 1;
                    shell_proc.pop_local_scope();
                    shell_proc.args = saved_args;
                    return (code, String::new());
                }
                Err(_) => shell_proc.last_exit,
            };
            shell_proc.depth -= 1;
            shell_proc.pop_local_scope();
            shell_proc.args = saved_args;
            let func_output = if capture {
                std::mem::take(&mut shell_proc.captured_output)
            } else {
                String::new()
            };
            return (exit, func_output);
        }
    }

    // Build a process for each stage
    let mut procs: Vec<Process> = (0..len).map(|_| shell_proc.fork()).collect();

    // Inherit stdin from shell_proc if present (used by CompoundPipeline).
    shell_proc.transfer_fd(STDIN, &mut procs[0]);

    // Apply per-command env prefixes
    for (i, env) in expanded_env.iter().enumerate() {
        for (k, v) in env {
            procs[i].set_env(k, v);
        }
    }

    // Connect pipes between adjacent stages
    for i in 0..len - 1 {
        let (tx, rx) = os::pipe(64);
        procs[i].set_channel_writer(STDOUT, tx);
        procs[i + 1].set_channel_reader(STDIN, rx);
    }

    // Last stage stdout: channel back to us
    let (last_tx, last_rx) = os::pipe(64);
    procs[len - 1].set_channel_writer(STDOUT, last_tx);

    // Every stage gets a stderr channel back to us
    let mut stderr_rxs = Vec::with_capacity(len);
    for p in &mut procs {
        let (tx, rx) = os::pipe(64);
        p.set_err_tx(tx.clone());
        p.set_channel_writer(STDERR, tx);
        stderr_rxs.push(rx);
    }

    // Apply redirections to each stage
    for (i, redirs) in expanded_redirects.iter().enumerate() {
        if let Err(e) = apply_redirects(&os, &mut procs[i], redirs).await {
            shell_proc.err_msg(&format!("strands-shell: {e}"));
            return (1, String::new());
        }
    }

    // Spawn each stage as a tokio task with CURRENT_PROCESS set
    // Use spawn_local so builtins (non-Send futures) can run in pipeline stages
    let mut handles = Vec::with_capacity(len);
    for (i, child) in procs.into_iter().enumerate() {
        if expanded_args[i].is_empty() {
            continue;
        }
        let name = expanded_args[i][0].clone();
        let args: Vec<String> = expanded_args[i][1..].to_vec();
        let os = os.clone();

        // Check for builtin, function, or script before spawning
        let builtin = builtins::lookup(&name);
        let func_body = child.get_function(&name).cloned();
        let is_script = name == "lash" || name == "sh";

        if builtin.is_some() || func_body.is_some() || is_script {
            // Builtins/functions: child has pipe fds (for I/O via CURRENT_PROCESS),
            // state_proc is a fork with env/functions but no fds (for builtin &mut Process)
            let mut state_proc = child.fork();
            handles.push(tokio::task::spawn_local(CURRENT_KERNEL.scope(
                os.clone(),
                CURRENT_PROCESS.scope(RefCell::new(child), async move {
                    if is_script {
                        if args.is_empty() {
                            return 0;
                        }
                        let exit = run_script(os, &mut state_proc, &args[0], &args[1..]).await;
                        let output = std::mem::take(&mut state_proc.captured_output);
                        if !output.is_empty()
                            && let Ok(mut w) = crate::io::stdout()
                        {
                            let _ = w.write_all(output.as_bytes()).await;
                        }
                        return exit;
                    }
                    if let Some(f) = builtin {
                        return match f(os.as_ref(), &mut state_proc, &args).await {
                            Ok(code) => code,
                            Err(e) => {
                                if let Ok(mut w) = crate::io::stderr() {
                                    let _ = w
                                        .write_all(
                                            format!("strands-shell: {name}: {e}\n").as_bytes(),
                                        )
                                        .await;
                                }
                                1
                            }
                        };
                    }
                    if let Some(body) = func_body {
                        state_proc.args = args;
                        state_proc.push_local_scope();
                        state_proc.capture = true;
                        // Transfer stdin from child (CURRENT_PROCESS) so the
                        // function body's commands can read pipeline input.
                        CURRENT_PROCESS.with(|p| {
                            p.borrow_mut().transfer_fd(STDIN, &mut state_proc);
                        });
                        let exit = match Box::pin(execute_command_line_inner(
                            os.clone(),
                            &mut state_proc,
                            &body,
                        ))
                        .await
                        {
                            Ok(code) => code,
                            Err(ControlFlow::Return(code)) => code,
                            Err(ControlFlow::Exit(code)) => code,
                            Err(_) => state_proc.last_exit,
                        };
                        state_proc.pop_local_scope();
                        // Write captured output to the pipeline's stdout
                        let output = std::mem::take(&mut state_proc.captured_output);
                        if !output.is_empty()
                            && let Ok(mut w) = crate::io::stdout()
                        {
                            let _ = w.write_all(output.as_bytes()).await;
                        }
                        return exit;
                    }
                    unreachable!()
                }),
            )));
        } else {
            // External commands
            handles.push(tokio::task::spawn_local(CURRENT_KERNEL.scope(
                os.clone(),
                CURRENT_PROCESS.scope(RefCell::new(child), async move {
                    match commands::lookup(&name) {
                        Some(f) => match f(os.as_ref(), &args).await {
                            Ok(code) => code,
                            Err(e) => {
                                if let Ok(mut w) = crate::io::stderr() {
                                    let _ = w
                                        .write_all(
                                            format!("strands-shell: {name}: {e}\n").as_bytes(),
                                        )
                                        .await;
                                }
                                1
                            }
                        },
                        None => {
                            if let Ok(mut w) = crate::io::stderr() {
                                let _ = w
                                    .write_all(
                                        format!("strands-shell: {name}: command not found\n")
                                            .as_bytes(),
                                    )
                                    .await;
                            }
                            127
                        }
                    }
                }),
            )));
        }
    }

    // Drain last stage stdout
    let max_output = shell_proc.max_output;
    let stdout_drain = if capture {
        let handle = tokio::spawn(async move {
            let mut reader = FdReader::from_receiver(last_rx);
            os::read_to_string_limited(&mut reader, max_output)
                .await
                .unwrap_or_default()
        });
        Some(handle)
    } else {
        let handle = tokio::spawn(async move {
            let mut reader = FdReader::from_receiver(last_rx);
            #[cfg(not(target_arch = "wasm32"))]
            {
                let mut stdout = tokio::io::stdout();
                let _ = tokio::io::copy(&mut reader, &mut stdout).await;
            }
            #[cfg(target_arch = "wasm32")]
            {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                let _ = reader.read_to_end(&mut buf).await;
                let _ = std::io::Write::write_all(&mut std::io::stdout(), &buf);
            }
            String::new()
        });
        Some(handle)
    };

    // Drain all stderr
    let stderr_drain = if capture {
        let handle = tokio::spawn(async move {
            let mut all = String::new();
            for rx in stderr_rxs {
                let mut reader = FdReader::from_receiver(rx);
                all.push_str(
                    &os::read_to_string_limited(&mut reader, max_output)
                        .await
                        .unwrap_or_default(),
                );
            }
            all
        });
        Some(handle)
    } else {
        let handle = tokio::spawn(async move {
            for rx in stderr_rxs {
                let mut reader = FdReader::from_receiver(rx);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut stderr = tokio::io::stderr();
                    let _ = tokio::io::copy(&mut reader, &mut stderr).await;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    use tokio::io::AsyncReadExt;
                    let mut buf = Vec::new();
                    let _ = reader.read_to_end(&mut buf).await;
                    let _ = std::io::Write::write_all(&mut std::io::stderr(), &buf);
                }
            }
            String::new()
        });
        Some(handle)
    };

    // Wait for all command tasks
    let mut last_exit = 0;
    for handle in handles {
        if let Ok(exit) = handle.await {
            last_exit = exit;
        }
    }

    let stdout_str = if let Some(h) = stdout_drain {
        h.await.unwrap_or_default()
    } else {
        String::new()
    };
    let stderr_str = if let Some(h) = stderr_drain {
        h.await.unwrap_or_default()
    } else {
        String::new()
    };
    if capture && !stderr_str.is_empty() {
        shell_proc.captured_stderr.push_str(&stderr_str);
    }

    (last_exit, stdout_str)
}
