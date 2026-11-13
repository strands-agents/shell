use super::date::format_date;
use super::ls::format_mode;
use crate::os::FileStat;
use crate::prelude::*;
use crate::vfs::{LASH_GID, LASH_UID};
use std::iter::Peekable;
use std::str::Chars;

const HELP: &str = "Usage: stat [OPTION]... FILE...
Display file status.

  -L, --dereference     follow links
  -c, --format=FORMAT   use FORMAT instead of the default; print a newline
                        after each use of FORMAT
      --printf=FORMAT   like --format, but interpret backslash escapes and
                        do not print a trailing newline
  -t, --terse           print the information in terse form

FORMAT sequences:
  %a  access rights in octal       %A  access rights in human readable form
  %b  number of 512-byte blocks    %B  size in bytes of each block (512)
  %d  device number in decimal     %D  device number in hex
  %f  raw mode in hex              %F  file type
  %g  group ID of owner            %G  group name of owner
  %h  number of hard links         %i  inode number
  %n  file name                    %N  quoted file name, with link target
  %o  optimal I/O transfer size    %s  total size, in bytes
  %t  major device type in hex     %T  minor device type in hex
  %u  user ID of owner             %U  user name of owner
  %w  time of birth, or -          %W  time of birth, seconds since Epoch
  %x  time of last access          %X  time of last access, seconds since Epoch
  %y  time of last modification    %Y  time of last modification, seconds since Epoch
  %z  time of last change          %Z  time of last change, seconds since Epoch

The kernel reports only the modification time, so the access and change times
repeat it and the birth time is unknown. The owner is always the shell user
(1000/lash), blocks are the size rounded up to 512 bytes (none for a symlink),
a directory has 2 links and anything else 1, and device numbers are 0. Files
in a directory bound from the host have inode number 0.";

const TERSE: &str = "%n %s %b %f %u %g %D %i %h %t %T %X %Y %Z %W %o";
const USER: &str = "lash";
const BLOCK_SIZE: u64 = 512;
const IO_BLOCK: u64 = 4096;
const MAX_FIELD: usize = u16::MAX as usize;

/// One operand and what the kernel said about it.
struct File<'a> {
    name: &'a str,
    st: &'a FileStat,
    /// The link target, for a symlink that was not followed.
    target: Option<String>,
}

#[command("stat")]
async fn cmd_stat(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut deref = false;
    let mut format: Option<String> = None;
    let mut escapes = false;
    let mut terse = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('L') | Long("dereference") => deref = true,
            Short('c') | Long("format") => {
                format = Some(parser.value()?.string()?);
                escapes = false;
            }
            Long("printf") => {
                format = Some(parser.value()?.string()?);
                escapes = true;
            }
            Short('t') | Long("terse") => terse = true,
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mut e = io::stderr()?;
    if files.is_empty() {
        wprintln!(e, "stat: missing operand")?;
        wprintln!(e, "Try 'stat --help' for more information.")?;
        return Ok(1);
    }
    let mut w = io::stdout()?;
    let mut code = 0;
    for name in &files {
        let st = if deref {
            io::stat(os, name).await
        } else {
            io::lstat(os, name).await
        };
        if !st.exists {
            wprintln!(e, "stat: cannot stat '{}': No such file or directory", name)?;
            code = 1;
            continue;
        }
        let target = if st.is_symlink {
            io::read_link(os, name).await.ok()
        } else {
            None
        };
        let file = File {
            name,
            st: &st,
            target,
        };
        let mut warnings = Vec::new();
        let expanded = match &format {
            Some(fmt) => expand(fmt, &file, escapes, &mut warnings),
            None if terse => expand(TERSE, &file, false, &mut warnings),
            None => Ok(default_output(&file).into_bytes()),
        };
        for warning in warnings {
            wprintln!(e, "stat: warning: {}", warning)?;
        }
        let mut out = match expanded {
            Ok(out) => out,
            Err(msg) => {
                wprintln!(e, "stat: {}", msg)?;
                return Ok(1);
            }
        };
        if !escapes && (format.is_some() || terse) {
            out.push(b'\n');
        }
        w.write_all(&out).await?;
    }
    Ok(code)
}

/// The multi-line report GNU `stat` prints without `-c` or `-t`.
fn default_output(f: &File) -> String {
    let st = f.st;
    let name = match &f.target {
        Some(target) => format!("{} -> {}", f.name, target),
        None => f.name.to_string(),
    };
    let links = if st.is_char_device || st.is_block_device {
        format!("{:<5} Device type: 0,0", links(st))
    } else {
        links(st).to_string()
    };
    let (mtime, _, _) = mtime(st);
    format!(
        "  File: {name}\n  Size: {:<10}\tBlocks: {:<10} IO Block: {:<6} {}\n\
         Device: {:x}h/{}d\tInode: {:<10}  Links: {links}\n\
         Access: ({:04o}/{:>10.10})  Uid: ({:5}/{:>8})   Gid: ({:5}/{:>8})\n\
         Access: {mtime}\nModify: {mtime}\nChange: {mtime}\n Birth: -\n",
        st.len,
        blocks(st),
        IO_BLOCK,
        file_type(st),
        st.dev,
        st.dev,
        st.ino,
        st.mode & 0o7777,
        format_mode(st.mode),
        LASH_UID,
        USER,
        LASH_GID,
        USER,
    )
}

/// The size rounded up to whole blocks; like a fast symlink on Linux, a
/// symlink takes none.
fn blocks(st: &FileStat) -> u64 {
    if st.is_symlink {
        0
    } else {
        st.len.div_ceil(BLOCK_SIZE)
    }
}

fn links(st: &FileStat) -> u32 {
    if st.is_dir { 2 } else { 1 }
}

fn file_type(st: &FileStat) -> &'static str {
    if st.is_symlink {
        "symbolic link"
    } else if st.is_dir {
        "directory"
    } else if st.is_char_device {
        "character special file"
    } else if st.is_block_device {
        "block special file"
    } else if st.is_fifo {
        "fifo"
    } else if st.is_socket {
        "socket"
    } else if st.len == 0 {
        "regular empty file"
    } else {
        "regular file"
    }
}

/// The modification time as GNU prints it (`2020-01-01 00:00:00.000000000
/// +0000`), and as seconds and nanoseconds since the epoch.
fn mtime(st: &FileStat) -> (String, i64, u32) {
    let since = st
        .modified
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    let secs = since.as_secs() as i64;
    let nanos = since.subsec_nanos();
    (
        format_date("%Y-%m-%d %H:%M:%S.%N %z", secs, nanos),
        secs,
        nanos,
    )
}

/// Quote a name like GNU `%N`: in single quotes, or double quotes when the
/// name has a single quote and nothing a double quote would expand.
fn quote(s: &str) -> String {
    if !s.contains('\'') {
        format!("'{}'", s)
    } else if !s.contains(['"', '$', '`', '\\']) {
        format!("\"{}\"", s)
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The text for one `%` conversion, and whether it is a number (which the
/// `0` flag pads with zeros).
///
/// Like GNU, a precision gives `%X`, `%Y` and `%Z` that many digits of
/// fractional seconds (9 when it has no digits) and pads other numbers with
/// leading zeros, and the `#` flag prefixes octal with `0` and hex with `0x`.
fn conversion(conv: char, f: &File, precision: Option<Option<usize>>, alt: bool) -> (String, bool) {
    let st = f.st;
    let (text, numeric) = match conv {
        'a' => (format!("{:o}", st.mode & 0o7777), true),
        'A' => (format_mode(st.mode), false),
        'b' => (blocks(st).to_string(), true),
        'B' => (BLOCK_SIZE.to_string(), true),
        'd' => (st.dev.to_string(), true),
        'D' => (format!("{:x}", st.dev), true),
        'f' => (format!("{:x}", st.mode), true),
        'F' => (file_type(st).to_string(), false),
        'g' => (LASH_GID.to_string(), true),
        'G' => (USER.to_string(), false),
        'h' => (links(st).to_string(), true),
        'i' => (st.ino.to_string(), true),
        'n' => (f.name.to_string(), false),
        'N' => match &f.target {
            Some(target) => (format!("{} -> {}", quote(f.name), quote(target)), false),
            None => (quote(f.name), false),
        },
        'o' => (IO_BLOCK.to_string(), true),
        's' => (st.len.to_string(), true),
        't' | 'T' => ("0".to_string(), true),
        'u' => (LASH_UID.to_string(), true),
        'U' => (USER.to_string(), false),
        'w' => ("-".to_string(), false),
        'W' => ("0".to_string(), true),
        'x' | 'y' | 'z' => (mtime(st).0, false),
        'X' | 'Y' | 'Z' => {
            let (_, secs, nanos) = mtime(st);
            let digits = match precision {
                Some(p) => p.unwrap_or(9),
                None => 0,
            };
            if digits == 0 {
                return (secs.to_string(), true);
            }
            let mut frac = format!("{nanos:09}");
            frac.truncate(digits);
            return (format!("{secs}.{frac:0<digits$}"), true);
        }
        _ => return ("?".to_string(), false),
    };
    if !numeric {
        return (text, false);
    }
    let text = match precision {
        Some(Some(p)) => format!("{text:0>p$}"),
        _ => text,
    };
    let text = match conv {
        'a' if alt && !text.starts_with('0') => format!("0{text}"),
        'D' | 'f' | 't' | 'T' if alt && text != "0" => format!("0x{text}"),
        _ => text,
    };
    (text, true)
}

/// Like GNU, `+` or a space puts a sign in front of a decimal number.
fn sign(conv: char, flags: &str, text: String) -> String {
    if matches!(conv, 'a' | 'D' | 'f' | 't' | 'T') {
        text
    } else if flags.contains('+') {
        format!("+{text}")
    } else if flags.contains(' ') {
        format!(" {text}")
    } else {
        text
    }
}

/// Expand `fmt` for one file, in a single pass so expanded text is never
/// re-scanned. With `escapes`, backslash sequences are interpreted too, and
/// each one GNU would warn about adds to `warnings`. A directive cut short by
/// the end of `fmt` is an error.
fn expand(
    fmt: &str,
    f: &File,
    escapes: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(fmt.len());
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && escapes {
            escape(&mut chars, &mut out, warnings);
            continue;
        }
        if c != '%' {
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        let mut flags = String::new();
        while let Some(&c) = chars.peek().filter(|c| "-#0 +'".contains(**c)) {
            flags.push(c);
            chars.next();
        }
        let mut directive = format!("%{flags}");
        let width = number(&mut chars, &mut directive);
        let mut precision = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            directive.push('.');
            let digits = directive.len();
            let p = number(&mut chars, &mut directive);
            precision = Some((directive.len() > digits).then_some(p));
        }
        let Some(conv) = chars.next() else {
            if directive.len() > 1 {
                return Err(format!("'{directive}': invalid directive"));
            }
            out.push(b'%');
            break;
        };
        if conv == '%' {
            out.push(b'%');
            continue;
        }
        let (text, numeric) = conversion(conv, f, precision, flags.contains('#'));
        let text = if numeric {
            sign(conv, &flags, text)
        } else {
            text
        };
        let pad = width.saturating_sub(text.chars().count());
        let padded = if flags.contains('-') {
            format!("{}{}", text, " ".repeat(pad))
        } else if flags.contains('0') && numeric {
            // Zeros go after a sign, as in `+0005`.
            let digits = text.trim_start_matches(['+', ' ']);
            let sign = &text[..text.len() - digits.len()];
            format!("{}{}{}", sign, "0".repeat(pad), digits)
        } else {
            format!("{}{}", " ".repeat(pad), text)
        };
        out.extend_from_slice(padded.as_bytes());
    }
    Ok(out)
}

/// Read the digits at the front of `chars` onto `directive` and return
/// their value. Like `printf`, a field width or precision is capped at
/// 65535 so a huge one can't exhaust memory.
fn number(chars: &mut Peekable<Chars>, directive: &mut String) -> usize {
    let mut n = 0usize;
    while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
        n = (n * 10 + d as usize).min(MAX_FIELD);
        directive.push(chars.next().unwrap());
    }
    n
}

/// Interpret the backslash sequence after a `\`, like GNU `stat --printf`.
/// An unknown escape prints the character after the backslash.
fn escape(chars: &mut Peekable<Chars>, out: &mut Vec<u8>, warnings: &mut Vec<String>) {
    let Some(c) = chars.next() else {
        warnings.push("backslash at end of format".to_string());
        out.push(b'\\');
        return;
    };
    let byte = match c {
        'a' => 0x07,
        'b' => 0x08,
        'e' => 0x1b,
        'f' => 0x0c,
        'n' => b'\n',
        'r' => b'\r',
        't' => b'\t',
        'v' => 0x0b,
        '0'..='7' => {
            let mut n = c.to_digit(8).unwrap();
            for _ in 0..2 {
                match chars.peek().and_then(|c| c.to_digit(8)) {
                    Some(d) => {
                        n = n * 8 + d;
                        chars.next();
                    }
                    None => break,
                }
            }
            n as u8
        }
        'x' if chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) => {
            let mut n = 0;
            for _ in 0..2 {
                match chars.peek().and_then(|c| c.to_digit(16)) {
                    Some(d) => {
                        n = n * 16 + d;
                        chars.next();
                    }
                    None => break,
                }
            }
            n as u8
        }
        other => {
            if !"\\\"'".contains(other) {
                warnings.push(format!("unrecognized escape '\\{}'", other));
            }
            out.extend_from_slice(other.encode_utf8(&mut [0; 4]).as_bytes());
            return;
        }
    };
    out.push(byte);
}
