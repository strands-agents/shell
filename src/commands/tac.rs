use crate::prelude::*;

const HELP: &str = "Usage: tac [OPTION]... [FILE]...
Write each FILE to standard output, last line first.
With no FILE, or when FILE is -, read standard input.

Options:
  -b, --before             attach the separator before instead of after
  -s, --separator=STRING   use STRING as the separator instead of newline";

/// Output is written in chunks of about this many bytes.
const CHUNK: usize = 8192;

/// Splits `data` into records that end with `sep`, or start with it when `before`.
///
/// Like GNU tac, the separator is searched from the end, which matters when it can
/// overlap itself (`-s aa` on `aaa`).
fn records<'a>(data: &'a [u8], sep: &[u8], before: bool) -> Vec<&'a [u8]> {
    let mut cuts = Vec::new();
    if !sep.is_empty() {
        let mut at = data.len().checked_sub(sep.len());
        while let Some(i) = at {
            if &data[i..i + sep.len()] == sep {
                cuts.push(if before { i } else { i + sep.len() });
                at = i.checked_sub(sep.len());
            } else {
                at = i.checked_sub(1);
            }
        }
    }
    let mut records = Vec::with_capacity(cuts.len() + 1);
    let mut start = 0;
    for &cut in cuts.iter().rev() {
        records.push(&data[start..cut]);
        start = cut;
    }
    records.push(&data[start..]);
    records
}

#[command("tac")]
async fn cmd_tac(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut before = false;
    let mut separator = "\n".to_string();
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('b') | Long("before") => before = true,
            Short('s') | Long("separator") => separator = parser.value()?.string()?,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if files.is_empty() {
        files.push("-".to_string());
    }
    let mut w = io::stdout()?;
    let mut status = 0;
    // Stderr is taken once, on the first error, so later errors are reported too.
    let mut stderr = None;
    let mut stdin_taken = false;
    for path in &files {
        let mut data = Vec::new();
        let read = async {
            let mut reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if path == "-" {
                // A second `-` finds standard input already read to the end.
                if std::mem::replace(&mut stdin_taken, true) {
                    Box::new(tokio::io::empty())
                } else {
                    Box::new(io::stdin()?)
                }
            } else {
                let fd = io::open(os, path, OpenFlags::read()).await?;
                Box::new(io::take_reader(fd)?)
            };
            reader.read_to_end(&mut data).await
        };
        if let Err(e) = read.await {
            let ew = match &mut stderr {
                Some(ew) => ew,
                None => stderr.insert(io::stderr()?),
            };
            wprintln!(ew, "tac: {path}: {e}")?;
            status = 1;
            continue;
        }
        let mut out = Vec::new();
        for record in records(&data, separator.as_bytes(), before)
            .into_iter()
            .rev()
        {
            out.extend_from_slice(record);
            if out.len() >= CHUNK {
                w.write_all(&out).await?;
                out.clear();
            }
        }
        w.write_all(&out).await?;
    }
    Ok(status)
}
