//! The shared body of the `*sum` checksum commands (`sha256sum`, `md5sum`):
//! printing digests of files and verifying them with `-c`, matching GNU
//! coreutils. Each command supplies an [`Algorithm`].

use digest::DynDigest;

use crate::os::{FdReader, FdWriter};
use crate::prelude::*;

/// A hash function and the names a checksum command shows for it.
pub struct Algorithm {
    /// The command name, used in messages, e.g. `sha256sum`.
    pub command: &'static str,
    /// The digest name GNU prints in `-c` warnings, e.g. `SHA256`.
    pub tag: &'static str,
    /// Creates a fresh hasher.
    pub new_hasher: fn() -> Box<dyn DynDigest + Send>,
}

/// Runs a checksum command with `algo`.
pub async fn run(os: &dyn Kernel, args: &[String], algo: &Algorithm) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut binary = None;
    let mut check = false;
    let mut quiet = false;
    let mut status = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('b') | Long("binary") => binary = Some(true),
            Short('t') | Long("text") => binary = Some(false),
            Short('c') | Long("check") => check = true,
            Long("quiet") => quiet = true,
            Long("status") => status = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", help(algo))?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if check && binary.is_some() {
        return Err(
            "the --binary and --text options are meaningless when verifying checksums".into(),
        );
    }
    if status && !check {
        return Err("the --status option is meaningful only when verifying checksums".into());
    }
    if quiet && !check {
        return Err("the --quiet option is meaningful only when verifying checksums".into());
    }
    if files.is_empty() {
        files.push("-".to_string());
    }

    let mut ctx = Context {
        os,
        algo,
        out: io::stdout()?,
        err: io::stderr()?,
        stdin: None,
    };
    let mut ok = true;
    if check {
        let mut format = LineFormat::Unknown;
        for file in &files {
            ok &= ctx.check_file(file, quiet, status, &mut format).await?;
        }
    } else {
        let marker = if binary == Some(true) { '*' } else { ' ' };
        for file in &files {
            let Some(digest) = ctx.digest_file(file).await? else {
                ok = false;
                continue;
            };
            let escape = file.contains(['\\', '\n']);
            let line = format!(
                "{}{} {}{}\n",
                if escape { "\\" } else { "" },
                digest,
                marker,
                escaped_name(file, escape)
            );
            ctx.out.write_all(line.as_bytes()).await?;
        }
    }
    Ok(if ok { 0 } else { 1 })
}

fn help(algo: &Algorithm) -> String {
    let bits = (algo.new_hasher)().output_size() * 8;
    format!(
        "Usage: {cmd} [OPTION]... [FILE]...
Print or check {tag} ({bits}-bit) checksums.
With no FILE, or when FILE is -, read standard input.

Options:
  -b, --binary  read in binary mode (marks the file with '*')
  -t, --text    read in text mode (default)
  -c, --check   read checksums from the FILEs and check them

When checking:
      --quiet   don't print OK for each successfully verified file
      --status  don't output anything, the exit status shows success",
        cmd = algo.command,
        tag = algo.tag,
    )
}

/// Whether `-c` lines carry a type marker before the name. GNU decides on the
/// first well-formed line and rejects lines in the other format after that.
#[derive(PartialEq)]
enum LineFormat {
    Unknown,
    /// `HASH  NAME` or `HASH *NAME`, as this command prints.
    Marked,
    /// `HASH NAME`, as printed by BSD `md5 -r`.
    Unmarked,
}

struct Context<'a> {
    os: &'a dyn Kernel,
    algo: &'a Algorithm,
    out: FdWriter,
    err: FdWriter,
    /// Stdin, taken the first time a file named `-` is read. A process can
    /// take it only once, so later reads of `-` reuse it and see its end.
    stdin: Option<FdReader>,
}

impl Context<'_> {
    /// Opens `file`, or stdin for `-`, reporting a failure to stderr.
    async fn open(&mut self, file: &str) -> std::io::Result<Option<FdReader>> {
        let opened = if file == "-" {
            match self.stdin.take() {
                Some(r) => Ok(r),
                None => io::stdin(),
            }
        } else {
            match io::open(self.os, file, OpenFlags::read()).await {
                Ok(fd) => io::take_reader(fd),
                Err(e) => Err(e),
            }
        };
        match opened {
            Ok(r) => Ok(Some(r)),
            Err(e) => {
                wprintln!(self.err, "{}: {}: {}", self.algo.command, file, e)?;
                Ok(None)
            }
        }
    }

    /// Hands a reader opened by [`Self::open`] back if it was stdin.
    fn close(&mut self, file: &str, r: FdReader) {
        if file == "-" {
            self.stdin = Some(r);
        }
    }

    /// Returns the lowercase hex digest of `file`, or `None` after reporting
    /// why it couldn't be read.
    async fn digest_file(&mut self, file: &str) -> std::io::Result<Option<String>> {
        let Some(mut r) = self.open(file).await? else {
            return Ok(None);
        };
        let mut hasher = (self.algo.new_hasher)();
        let mut buf = [0u8; 8192];
        let result = loop {
            match r.read(&mut buf).await {
                Ok(0) => break Ok(()),
                Ok(n) => hasher.update(&buf[..n]),
                Err(e) => break Err(e),
            }
        };
        self.close(file, r);
        if let Err(e) = result {
            wprintln!(self.err, "{}: {}: {}", self.algo.command, file, e)?;
            return Ok(None);
        }
        let hex = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Some(hex))
    }

    /// Verifies every checksum line in `list`. Returns whether the list had
    /// well-formed lines and every listed file was read and matched.
    async fn check_file(
        &mut self,
        list: &str,
        quiet: bool,
        status: bool,
        format: &mut LineFormat,
    ) -> std::io::Result<bool> {
        let Some(r) = self.open(list).await? else {
            return Ok(false);
        };
        let mut reader = BufReader::new(r);
        let mut lines = Vec::new();
        let mut line = Vec::new();
        let read = loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) => break Ok(()),
                Ok(_) => lines.push(std::mem::take(&mut line)),
                Err(e) => break Err(e),
            }
        };
        self.close(list, reader.into_inner());
        let shown = if list == "-" { "standard input" } else { list };
        if read.is_err() {
            wprintln!(self.err, "{}: {}: read error", self.algo.command, shown)?;
            return Ok(false);
        }

        let hex_len = (self.algo.new_hasher)().output_size() * 2;
        let mut well_formed = false;
        let mut misformatted = 0;
        let mut unreadable = 0;
        let mut mismatched = 0;
        for line in &lines {
            if line.first() == Some(&b'#') {
                continue;
            }
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            let Some((expected, name)) = parse_line(line, hex_len, format) else {
                misformatted += 1;
                continue;
            };
            if list == "-" && name == "-" {
                misformatted += 1;
                continue;
            }
            well_formed = true;
            let escape = !status && name.contains('\n');
            let shown_name = format!(
                "{}{}",
                if escape { "\\" } else { "" },
                escaped_name(&name, escape)
            );
            let Some(actual) = self.digest_file(&name).await? else {
                unreadable += 1;
                if !status {
                    wprintln!(self.out, "{}: FAILED open or read", shown_name)?;
                }
                continue;
            };
            let matched = actual.eq_ignore_ascii_case(expected);
            if !matched {
                mismatched += 1;
            }
            if !status && !matched {
                wprintln!(self.out, "{}: FAILED", shown_name)?;
            } else if !status && !quiet {
                wprintln!(self.out, "{}: OK", shown_name)?;
            }
        }

        let cmd = self.algo.command;
        if !well_formed {
            wprintln!(
                self.err,
                "{}: {}: no properly formatted {} checksum lines found",
                cmd,
                shown,
                self.algo.tag
            )?;
            return Ok(false);
        }
        if !status {
            if misformatted > 0 {
                let what = plural(misformatted, "line is", "lines are");
                wprintln!(
                    self.err,
                    "{cmd}: WARNING: {misformatted} {what} improperly formatted"
                )?;
            }
            if unreadable > 0 {
                let what = plural(unreadable, "listed file", "listed files");
                wprintln!(
                    self.err,
                    "{cmd}: WARNING: {unreadable} {what} could not be read"
                )?;
            }
            if mismatched > 0 {
                let what = plural(mismatched, "computed checksum", "computed checksums");
                wprintln!(
                    self.err,
                    "{cmd}: WARNING: {mismatched} {what} did NOT match"
                )?;
            }
        }
        Ok(unreadable == 0 && mismatched == 0)
    }
}

/// Splits a `-c` line (without its newline) into the expected hex digest and
/// the file name, following GNU's `split_3`. Leading blanks are skipped, and
/// a leading backslash means the name is escaped. Returns `None` for a line
/// that isn't well formed.
fn parse_line<'a>(
    line: &'a [u8],
    hex_len: usize,
    format: &mut LineFormat,
) -> Option<(&'a str, String)> {
    let mut i = line
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let escaped = line.get(i) == Some(&b'\\');
    if escaped {
        i += 1;
    }
    let min_len = hex_len + 2 + usize::from(line.get(i) == Some(&b'\\'));
    if line.len() - i < min_len {
        return None;
    }
    let hex = &line[i..i + hex_len];
    i += hex_len;
    if line[i] != b' ' && line[i] != b'\t' {
        return None;
    }
    i += 1;
    if !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    if line.len() - i == 1 || (line[i] != b' ' && line[i] != b'*') {
        if *format == LineFormat::Marked {
            return None;
        }
        *format = LineFormat::Unmarked;
    } else if *format != LineFormat::Unmarked {
        *format = LineFormat::Marked;
        i += 1;
    }
    let name = &line[i..];
    let name = if escaped {
        unescape(name)?
    } else {
        name.to_vec()
    };
    let hex = std::str::from_utf8(hex).expect("hex digits are ASCII");
    Some((hex, String::from_utf8_lossy(&name).into_owned()))
}

/// Undoes [`escaped_name`]: `\\` becomes a backslash and `\n` a newline. Any
/// other escape, or a trailing backslash, makes the name invalid.
fn unescape(name: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(name.len());
    let mut bytes = name.iter();
    while let Some(&b) = bytes.next() {
        match b {
            b'\\' => match bytes.next() {
                Some(b'n') => out.push(b'\n'),
                Some(b'\\') => out.push(b'\\'),
                _ => return None,
            },
            _ => out.push(b),
        }
    }
    Some(out)
}

/// Writes backslashes as `\\` and newlines as `\n` when `escape` is set, so a
/// name always fits on one line. GNU marks such lines with a leading `\`.
fn escaped_name(name: &str, escape: bool) -> String {
    if escape {
        name.replace('\\', "\\\\").replace('\n', "\\n")
    } else {
        name.to_string()
    }
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}
