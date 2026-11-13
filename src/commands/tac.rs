use crate::prelude::*;

const HELP: &str = "Usage: tac [OPTION]... [FILE]...
Write each FILE to standard output, last line first.
With no FILE, or when FILE is -, read standard input.

Options:
  -b, --before             attach the separator before instead of after
  -s, --separator=STRING   use STRING as the separator instead of newline";

/// Splits `data` into records that end with `sep`, or start with it when `before`.
fn records<'a>(data: &'a [u8], sep: &[u8], before: bool) -> Vec<&'a [u8]> {
    let mut records = Vec::new();
    if sep.is_empty() {
        records.push(data);
        return records;
    }
    let mut start = 0;
    let mut i = 0;
    while i + sep.len() <= data.len() {
        if &data[i..i + sep.len()] == sep {
            let end = if before { i } else { i + sep.len() };
            records.push(&data[start..end]);
            start = end;
            i += sep.len();
        } else {
            i += 1;
        }
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
    for path in &files {
        let mut data = Vec::new();
        let read = async {
            let mut reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if path == "-" {
                Box::new(io::stdin()?)
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
        let reversed: Vec<u8> = records(&data, separator.as_bytes(), before)
            .into_iter()
            .rev()
            .flatten()
            .copied()
            .collect();
        w.write_all(&reversed).await?;
    }
    Ok(status)
}
