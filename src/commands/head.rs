use crate::prelude::*;
use std::collections::VecDeque;
use std::ffi::OsString;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite};

const HELP: &str = "Usage: head [-n LINES | -c BYTES] [FILE]
Output the first part of files.

Options:
  -n, --lines LINES   number of lines to show (default: 10); -N means all but the last N
  -c, --bytes BYTES   number of bytes to show; -N means all but the last N";

#[command("head")]
async fn cmd_head(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(legacy_line_count(args));
    let mut n: usize = 10;
    let mut bytes: Option<u64> = None;
    let mut all_but_last = false;
    let mut file = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('n') | Long("lines") => {
                (all_but_last, n) = signed_count(parser.value()?)?;
                bytes = None;
            }
            Short('c') | Long("bytes") => {
                let (negative, c) = signed_count(parser.value()?)?;
                all_but_last = negative;
                bytes = Some(c);
            }
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) if file.is_none() => file = Some(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mut w = io::stdout()?;
    let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if let Some(path) = &file {
        let fd = io::open(os, path, OpenFlags::read()).await?;
        Box::new(io::take_reader(fd)?)
    } else {
        Box::new(io::stdin()?)
    };
    if let Some(c) = bytes {
        if all_but_last {
            return write_all_but_last_bytes(reader, c, &mut w).await;
        }
        tokio::io::copy(&mut reader.take(c), &mut w).await?;
        return Ok(0);
    }
    let mut reader = BufReader::new(reader);
    if all_but_last {
        return write_all_but_last_lines(reader, n, &mut w).await;
    }
    let mut line = String::new();
    for _ in 0..n {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        w.write_all(line.as_bytes()).await?;
    }
    Ok(0)
}

/// Parses a `-n`/`-c` value, where a leading `-` asks for all but the last N.
fn signed_count<T: std::str::FromStr>(value: OsString) -> Result<(bool, T), lexopt::Error>
where
    T::Err: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    match value.to_str().and_then(|s| s.strip_prefix('-')) {
        Some(rest) => match rest.parse() {
            Ok(count) => Ok((true, count)),
            Err(error) => Err(lexopt::Error::ParsingFailed {
                value: value.to_string_lossy().into_owned(),
                error: error.into(),
            }),
        },
        None => Ok((false, value.parse()?)),
    }
}

/// Copies everything but the last `keep` lines, holding back only those lines.
async fn write_all_but_last_lines(
    mut reader: impl AsyncBufRead + Unpin,
    keep: usize,
    w: &mut (impl AsyncWrite + Unpin),
) -> CommandResult {
    let mut held: VecDeque<Vec<u8>> = VecDeque::new();
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Ok(0);
        }
        held.push_back(line);
        if held.len() > keep
            && let Some(line) = held.pop_front()
        {
            w.write_all(&line).await?;
        }
    }
}

/// Copies everything but the last `keep` bytes, holding back only those bytes.
async fn write_all_but_last_bytes(
    mut reader: impl AsyncRead + Unpin,
    keep: u64,
    w: &mut (impl AsyncWrite + Unpin),
) -> CommandResult {
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    let mut held = Vec::new();
    let mut chunk = vec![0; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(0);
        }
        held.extend_from_slice(&chunk[..read]);
        if held.len() > keep {
            let excess = held.len() - keep;
            w.write_all(&held[..excess]).await?;
            held.drain(..excess);
        }
    }
}

/// Rewrites a leading obsolete `-N` count into `-n N`, as GNU `head` and `tail` accept it.
///
/// Only the first argument is considered, so an operand after `--` or an option's value is
/// never taken for a count.
pub(super) fn legacy_line_count(args: &[String]) -> Vec<String> {
    let count = args
        .first()
        .and_then(|first| first.strip_prefix('-'))
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    match count {
        Some(digits) => ["-n", digits]
            .into_iter()
            .map(String::from)
            .chain(args[1..].iter().cloned())
            .collect(),
        None => args.to_vec(),
    }
}
