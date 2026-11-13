use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use similar::{Algorithm, DiffOp, capture_diff_slices};

use super::date::format_date;
use crate::os::{FdWriter, FileStat};
use crate::prelude::*;

const HELP: &str = "Usage: diff [OPTION]... FILE1 FILE2
Compare files line by line.

Options:
  -u, -U NUM, --unified[=NUM]  output NUM (default 3) lines of unified context
  -q, --brief                  report only when files differ
  -r, --recursive              recursively compare any subdirectories found
  -N, --new-file               treat absent files as empty
  -h, --help                   display this help

FILE1 or FILE2 can be a directory. A FILE of '-' means standard input.
Exit status is 0 if inputs are the same, 1 if different, 2 if trouble.";

const TRY_HELP: &str = "diff: Try 'diff --help' for more information.";

const UNIFIED_CONTEXT: usize = 3;

const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S.%N %z";

/// How one comparison ended. The command exits with the worst one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Same = 0,
    Differ = 1,
    Trouble = 2,
}

struct Options {
    /// Lines of unified context, or `None` for the normal format.
    context: Option<usize>,
    brief: bool,
    recursive: bool,
    new_file: bool,
    /// The options as given, which the `diff ...` line before each file in a
    /// directory comparison repeats.
    switches: String,
}

/// One side of a file comparison.
struct Input {
    name: String,
    data: Vec<u8>,
    modified: SystemTime,
}

/// A run of lines that differ: `old_len` lines at `old` became `new_len`
/// lines at `new`, both 0-based.
struct Change {
    old: usize,
    old_len: usize,
    new: usize,
    new_len: usize,
}

struct Diff<'a> {
    os: &'a dyn Kernel,
    opts: Options,
    out: FdWriter,
    err: FdWriter,
    /// Standard input, read on first use so `diff - -` reads it once.
    stdin: Option<Vec<u8>>,
}

/// GNU's wording for the errors diff reports about a path.
fn describe(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        std::io::ErrorKind::IsADirectory => "Is a directory".to_string(),
        _ => e.to_string(),
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn changes(old: &[&[u8]], new: &[&[u8]]) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        let (o, old_len, n, new_len) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index, old_len, new_index, 0),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index, 0, new_index, new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (old_index, old_len, new_index, new_len),
        };
        match out.last_mut() {
            Some(c) if c.old + c.old_len == o && c.new + c.new_len == n => {
                c.old_len += old_len;
                c.new_len += new_len;
            }
            _ => out.push(Change {
                old: o,
                old_len,
                new: n,
                new_len,
            }),
        }
    }
    out
}

/// Appends a line with its prefix, marking a last line that has no newline.
fn push_line(out: &mut Vec<u8>, prefix: &[u8], line: &[u8]) {
    out.extend_from_slice(prefix);
    out.extend_from_slice(line);
    if !line.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
}

/// A normal-format line range: `first` or `first,last`, 1-based.
fn normal_range(start: usize, len: usize) -> String {
    if len == 1 {
        format!("{}", start + 1)
    } else {
        format!("{},{}", start + 1, start + len)
    }
}

/// A unified hunk range: `first`, `first,len`, or `before,0` when empty.
fn unified_range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{len}", start + 1),
    }
}

fn normal(out: &mut Vec<u8>, old: &[&[u8]], new: &[&[u8]], changes: &[Change]) {
    for c in changes {
        let header = if c.old_len == 0 {
            format!("{}a{}\n", c.old, normal_range(c.new, c.new_len))
        } else if c.new_len == 0 {
            format!("{}d{}\n", normal_range(c.old, c.old_len), c.new)
        } else {
            format!(
                "{}c{}\n",
                normal_range(c.old, c.old_len),
                normal_range(c.new, c.new_len)
            )
        };
        out.extend_from_slice(header.as_bytes());
        for line in &old[c.old..c.old + c.old_len] {
            push_line(out, b"< ", line);
        }
        if c.old_len > 0 && c.new_len > 0 {
            out.extend_from_slice(b"---\n");
        }
        for line in &new[c.new..c.new + c.new_len] {
            push_line(out, b"> ", line);
        }
    }
}

fn timestamp(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    format_date(TIME_FORMAT, since.as_secs() as i64, since.subsec_nanos())
}

fn unified(out: &mut Vec<u8>, a: &Input, b: &Input, context: usize) {
    let old: Vec<&[u8]> = a.data.split_inclusive(|&c| c == b'\n').collect();
    let new: Vec<&[u8]> = b.data.split_inclusive(|&c| c == b'\n').collect();
    let changes = changes(&old, &new);
    let header = format!(
        "--- {}\t{}\n+++ {}\t{}\n",
        a.name,
        timestamp(a.modified),
        b.name,
        timestamp(b.modified)
    );
    out.extend_from_slice(header.as_bytes());
    let mut i = 0;
    while i < changes.len() {
        // Changes no more than two contexts apart share a hunk.
        let mut j = i;
        while j + 1 < changes.len()
            && changes[j + 1].old - (changes[j].old + changes[j].old_len) <= 2 * context
        {
            j += 1;
        }
        let (first, last) = (&changes[i], &changes[j]);
        let lead = first.old.min(context);
        let trail = (old.len() - (last.old + last.old_len)).min(context);
        let old_start = first.old - lead;
        let old_end = last.old + last.old_len + trail;
        let new_start = first.new - lead;
        let new_end = last.new + last.new_len + trail;
        let hunk = format!(
            "@@ -{} +{} @@\n",
            unified_range(old_start, old_end - old_start),
            unified_range(new_start, new_end - new_start)
        );
        out.extend_from_slice(hunk.as_bytes());
        let mut pos = old_start;
        for c in &changes[i..=j] {
            for line in &old[pos..c.old] {
                push_line(out, b" ", line);
            }
            for line in &old[c.old..c.old + c.old_len] {
                push_line(out, b"-", line);
            }
            for line in &new[c.new..c.new + c.new_len] {
                push_line(out, b"+", line);
            }
            pos = c.old + c.old_len;
        }
        for line in &old[pos..old_end] {
            push_line(out, b" ", line);
        }
        i = j + 1;
    }
}

impl Diff<'_> {
    async fn error(
        &mut self,
        msg: &str,
    ) -> Result<Status, Box<dyn std::error::Error + Send + Sync>> {
        wprintln!(self.err, "diff: {msg}")?;
        Ok(Status::Trouble)
    }

    async fn say(&mut self, msg: &str) -> Result<Status, Box<dyn std::error::Error + Send + Sync>> {
        wprintln!(self.out, "{msg}")?;
        Ok(Status::Differ)
    }

    /// Stats `path`, reporting it when it can't be looked up.
    async fn stat(&mut self, path: &str) -> std::io::Result<FileStat> {
        match crate::io::try_stat(self.os, path).await {
            Ok(stat) => Ok(stat),
            Err(e) => {
                wprintln!(self.err, "diff: {path}: {}", describe(&e))?;
                Err(e)
            }
        }
    }

    async fn read(&mut self, path: &str, modified: Option<SystemTime>) -> std::io::Result<Input> {
        let max_output = io::with_process(|p| p.max_output);
        let (data, modified) = if path == "-" {
            if self.stdin.is_none() {
                let mut r = io::stdin()?;
                self.stdin = Some(crate::os::read_to_end_limited(&mut r, max_output).await?);
            }
            (self.stdin.clone().unwrap_or_default(), self.os.now())
        } else {
            let fd = io::open(self.os, path, OpenFlags::read()).await?;
            let mut r = io::take_reader(fd)?;
            let data = crate::os::read_to_end_limited(&mut r, max_output).await?;
            (data, modified.unwrap_or(UNIX_EPOCH))
        };
        Ok(Input {
            name: path.to_string(),
            data,
            modified,
        })
    }

    /// Reads `path`, or an empty input when it is absent and `-N` treats it
    /// as empty.
    async fn read_or_empty(
        &mut self,
        path: &str,
        stat: Option<&FileStat>,
    ) -> Result<Option<Input>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(stat) = stat else {
            return Ok(Some(Input {
                name: path.to_string(),
                data: Vec::new(),
                modified: UNIX_EPOCH,
            }));
        };
        match self.read(path, stat.modified).await {
            Ok(input) => Ok(Some(input)),
            Err(e) => {
                wprintln!(self.err, "diff: {path}: {}", describe(&e))?;
                Ok(None)
            }
        }
    }

    /// Compares two files. `in_dir` prints the `diff ...` line that names
    /// each pair in a directory comparison.
    async fn files(
        &mut self,
        a: &str,
        sa: Option<&FileStat>,
        b: &str,
        sb: Option<&FileStat>,
        in_dir: bool,
    ) -> Result<Status, Box<dyn std::error::Error + Send + Sync>> {
        let (Some(a), Some(b)) = (
            self.read_or_empty(a, sa).await?,
            self.read_or_empty(b, sb).await?,
        ) else {
            return Ok(Status::Trouble);
        };
        if a.data == b.data {
            return Ok(Status::Same);
        }
        if self.opts.brief {
            return self
                .say(&format!("Files {} and {} differ", a.name, b.name))
                .await;
        }
        if a.data.contains(&0) || b.data.contains(&0) {
            return self
                .say(&format!("Binary files {} and {} differ", a.name, b.name))
                .await;
        }
        let mut out = Vec::new();
        if in_dir {
            let line = format!("diff{} {} {}\n", self.opts.switches, a.name, b.name);
            out.extend_from_slice(line.as_bytes());
        }
        match self.opts.context {
            Some(context) => unified(&mut out, &a, &b, context),
            None => {
                let old: Vec<&[u8]> = a.data.split_inclusive(|&c| c == b'\n').collect();
                let new: Vec<&[u8]> = b.data.split_inclusive(|&c| c == b'\n').collect();
                normal(&mut out, &old, &new, &changes(&old, &new));
            }
        }
        self.out.write_all(&out).await?;
        Ok(Status::Differ)
    }

    /// Lists `dir`, or nothing when it is absent and `-N` treats it as empty.
    async fn list(&mut self, dir: &str) -> std::io::Result<BTreeSet<String>> {
        if self.opts.new_file && !io::stat(self.os, dir).await.exists {
            return Ok(BTreeSet::new());
        }
        match io::list_dir(self.os, dir).await {
            Ok(entries) => Ok(entries.into_iter().map(|e| e.name).collect()),
            Err(e) => {
                wprintln!(self.err, "diff: {dir}: {}", describe(&e))?;
                Err(e)
            }
        }
    }

    async fn dirs(
        &mut self,
        a: &str,
        b: &str,
    ) -> Result<Status, Box<dyn std::error::Error + Send + Sync>> {
        let (Ok(in_a), Ok(in_b)) = (self.list(a).await, self.list(b).await) else {
            return Ok(Status::Trouble);
        };
        let mut status = Status::Same;
        for name in in_a.union(&in_b) {
            let (pa, pb) = (join(a, name), join(b, name));
            let (has_a, has_b) = (in_a.contains(name), in_b.contains(name));
            if has_a != has_b && !self.opts.new_file {
                let dir = if has_a { a } else { b };
                status = status.max(self.say(&format!("Only in {dir}: {name}")).await?);
                continue;
            }
            let sa = if has_a {
                self.stat(&pa).await.ok()
            } else {
                None
            };
            let sb = if has_b {
                self.stat(&pb).await.ok()
            } else {
                None
            };
            if (has_a && sa.is_none()) || (has_b && sb.is_none()) {
                status = Status::Trouble;
                continue;
            }
            // With -N, an absent entry takes the kind of the one present.
            let a_dir = sa.as_ref().or(sb.as_ref()).is_some_and(|s| s.is_dir);
            let b_dir = sb.as_ref().or(sa.as_ref()).is_some_and(|s| s.is_dir);
            let result = match (a_dir, b_dir) {
                (true, true) if self.opts.recursive => Box::pin(self.dirs(&pa, &pb)).await?,
                (true, true) => {
                    wprintln!(self.out, "Common subdirectories: {pa} and {pb}")?;
                    Status::Same
                }
                (false, false) => self.files(&pa, sa.as_ref(), &pb, sb.as_ref(), true).await?,
                (true, false) => {
                    self.say(&format!(
                        "File {pa} is a directory while file {pb} is a regular file"
                    ))
                    .await?
                }
                (false, true) => {
                    self.say(&format!(
                        "File {pa} is a regular file while file {pb} is a directory"
                    ))
                    .await?
                }
            };
            status = status.max(result);
        }
        Ok(status)
    }

    /// Stats an operand. Standard input has no metadata to look up.
    async fn operand(&self, path: &str) -> std::io::Result<FileStat> {
        if path == "-" {
            return Ok(FileStat::default());
        }
        crate::io::try_stat(self.os, path).await
    }

    async fn run(
        &mut self,
        a: &str,
        b: &str,
    ) -> Result<Status, Box<dyn std::error::Error + Send + Sync>> {
        if a == "-" && b == "-" {
            return Ok(Status::Same);
        }
        let absent = |r: &std::io::Result<FileStat>| {
            r.as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        };
        let (ra, rb) = (self.operand(a).await, self.operand(b).await);
        // -N lets one absent operand stand for an empty file.
        let (mut stat_a, mut stat_b) = match (ra, rb) {
            (Ok(sa), Ok(sb)) => (Some(sa), Some(sb)),
            (ra, rb) if self.opts.new_file && absent(&ra) != absent(&rb) => (ra.ok(), rb.ok()),
            (ra, rb) => {
                for (path, r) in [(a, ra), (b, rb)] {
                    if let Err(e) = r {
                        wprintln!(self.err, "diff: {path}: {}", describe(&e))?;
                    }
                }
                return Ok(Status::Trouble);
            }
        };
        let a_dir = stat_a.as_ref().is_some_and(|s| s.is_dir);
        let b_dir = stat_b.as_ref().is_some_and(|s| s.is_dir);
        if a_dir && b_dir {
            return self.dirs(a, b).await;
        }
        if (a_dir && b == "-") || (b_dir && a == "-") {
            return self.error("cannot compare '-' to a directory").await;
        }
        // A file compared with a directory pairs with the same name in it.
        let (mut a, mut b) = (a.to_string(), b.to_string());
        if a_dir {
            a = join(&a, basename(&b));
            stat_a = self.stat(&a).await.ok();
            if stat_a.is_none() {
                return Ok(Status::Trouble);
            }
        } else if b_dir {
            b = join(&b, basename(&a));
            stat_b = self.stat(&b).await.ok();
            if stat_b.is_none() {
                return Ok(Status::Trouble);
            }
        }
        self.files(&a, stat_a.as_ref(), &b, stat_b.as_ref(), false)
            .await
    }
}

fn parse_context(value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("invalid context length '{value}'"))
}

/// Parses the arguments into options and operands, or a message for a usage
/// error.
fn parse(args: &[String]) -> Result<Option<(Options, Vec<String>)>, String> {
    let mut parser = lexopt::Parser::from_args(args);
    let mut context: Option<usize> = None;
    let mut brief = false;
    let mut recursive = false;
    let mut new_file = false;
    let mut operands = Vec::new();
    // Like GNU, repeated context options keep the largest.
    let mut widen = |n: usize| context = Some(context.map_or(n, |c| c.max(n)));
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Short('u') => widen(UNIFIED_CONTEXT),
            Short('U') => {
                let v = parser.value().map_err(|e| e.to_string())?;
                widen(parse_context(&v.to_string_lossy())?);
            }
            Long("unified") => match parser.optional_value() {
                Some(v) => widen(parse_context(&v.to_string_lossy())?),
                None => widen(UNIFIED_CONTEXT),
            },
            Short('q') | Long("brief") => brief = true,
            Short('r') | Long("recursive") => recursive = true,
            Short('N') | Long("new-file") => new_file = true,
            Short('h') | Long("help") => return Ok(None),
            Value(val) => operands.push(val.string().map_err(|e| e.to_string())?),
            Short(c) => return Err(format!("invalid option -- '{c}'")),
            Long(name) => return Err(format!("unrecognized option '--{name}'")),
        }
    }
    match operands.len() {
        0 => return Err("missing operand after 'diff'".to_string()),
        1 => return Err(format!("missing operand after '{}'", operands[0])),
        2 => {}
        _ => return Err(format!("extra operand '{}'", operands[2])),
    }
    // The options are the arguments that aren't operands, as typed.
    let mut switches: Vec<&String> = args.iter().collect();
    for operand in operands.iter().rev() {
        if let Some(i) = switches.iter().rposition(|a| *a == operand) {
            switches.remove(i);
        }
    }
    let switches = switches.iter().map(|s| format!(" {s}")).collect();
    let opts = Options {
        context,
        brief,
        recursive,
        new_file,
        switches,
    };
    Ok(Some((opts, operands)))
}

#[command("diff")]
async fn cmd_diff(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut err = io::stderr()?;
    let (opts, operands) = match parse(args) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let mut w = io::stdout()?;
            wprintln!(w, "{}", HELP)?;
            return Ok(0);
        }
        Err(msg) => {
            wprintln!(err, "diff: {msg}\n{TRY_HELP}")?;
            return Ok(2);
        }
    };
    let mut diff = Diff {
        os,
        opts,
        out: io::stdout()?,
        err,
        stdin: None,
    };
    match diff.run(&operands[0], &operands[1]).await {
        Ok(status) => Ok(status as i32),
        Err(e) => {
            wprintln!(diff.err, "diff: {e}")?;
            Ok(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{timestamp, unified_range};

    #[test]
    fn timestamp_matches_gnu_header() {
        // `TZ=UTC date -d @1762428896.123456789 '+%Y-%m-%d %H:%M:%S.%N %z'`.
        let time = UNIX_EPOCH + Duration::new(1762428896, 123456789);
        assert_eq!(timestamp(time), "2025-11-06 11:34:56.123456789 +0000");
    }

    #[test]
    fn unified_range_omits_a_length_of_one() {
        assert_eq!(unified_range(0, 0), "0,0");
        assert_eq!(unified_range(9, 0), "9,0");
        assert_eq!(unified_range(4, 1), "5");
        assert_eq!(unified_range(0, 10), "1,10");
    }
}
