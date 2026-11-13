use crate::prelude::*;

const HELP: &str = "Usage: rev [FILE]...
Reverse the characters of each line.
With no FILE, read standard input.";

/// Reverses `line` by UTF-8 character, keeping each invalid byte as it is.
fn reverse(line: &[u8]) -> Vec<u8> {
    let mut units: Vec<&[u8]> = Vec::with_capacity(line.len());
    for chunk in line.utf8_chunks() {
        let valid = chunk.valid();
        units.extend(
            valid
                .char_indices()
                .map(|(i, c)| &valid.as_bytes()[i..i + c.len_utf8()]),
        );
        units.extend(chunk.invalid().chunks(1));
    }
    units.into_iter().rev().flatten().copied().collect()
}

async fn rev_stream(
    reader: impl tokio::io::AsyncRead + Unpin,
    w: &mut crate::os::FdWriter,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Ok(());
        }
        let newline = line.last() == Some(&b'\n');
        let mut out = reverse(if newline {
            &line[..line.len() - 1]
        } else {
            &line
        });
        if newline {
            out.push(b'\n');
        }
        w.write_all(&out).await?;
    }
}

#[command("rev")]
async fn cmd_rev(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mut w = io::stdout()?;
    if files.is_empty() {
        rev_stream(io::stdin()?, &mut w).await?;
        return Ok(0);
    }
    let mut status = 0;
    // Stderr is taken once, on the first error, so later errors are reported too.
    let mut stderr = None;
    for path in &files {
        let reader = match io::open(os, path, OpenFlags::read()).await {
            Ok(fd) => io::take_reader(fd)?,
            Err(e) => {
                let ew = match &mut stderr {
                    Some(ew) => ew,
                    None => stderr.insert(io::stderr()?),
                };
                wprintln!(ew, "rev: cannot open {path}: {e}")?;
                status = 1;
                continue;
            }
        };
        rev_stream(reader, &mut w).await?;
    }
    Ok(status)
}
