use crate::prelude::*;

const HELP: &str = "Usage: head [-n LINES | -c BYTES] [FILE]
Output the first part of files.

Options:
  -n, --lines LINES   number of lines to show (default: 10)
  -c, --bytes BYTES   number of bytes to show";

#[command("head")]
async fn cmd_head(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(legacy_line_count(args));
    let mut n: usize = 10;
    let mut bytes: Option<u64> = None;
    let mut file = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('n') | Long("lines") => {
                n = parser.value()?.parse()?;
                bytes = None;
            }
            Short('c') | Long("bytes") => bytes = Some(parser.value()?.parse()?),
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
        tokio::io::copy(&mut reader.take(c), &mut w).await?;
        return Ok(0);
    }
    let mut reader = BufReader::new(reader);
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
