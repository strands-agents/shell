use crate::prelude::*;

const HELP: &str = "Usage: cat [-n] [FILE]...
Concatenate FILE(s) to standard output.
With no FILE, read standard input.

Options:
  -n    number all output lines";

/// Where copying one input stopped: reading it, or writing to stdout.
enum Failure {
    Read(std::io::Error),
    Write(std::io::Error),
}

async fn cat_stream<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    w: &mut crate::os::FdWriter,
    number: bool,
    lineno: &mut usize,
) -> Result<(), Failure> {
    if !number {
        let mut buf = vec![0; 8192];
        loop {
            let n = r.read(&mut buf).await.map_err(Failure::Read)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).await.map_err(Failure::Write)?;
        }
        return w.flush().await.map_err(Failure::Write);
    }
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.map_err(Failure::Read)? == 0 {
            break;
        }
        *lineno += 1;
        wprint!(w, "{:>6}\t{}", lineno, line).map_err(Failure::Write)?;
    }
    Ok(())
}

#[command("cat")]
async fn cmd_cat(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut files = Vec::new();
    let mut number = false;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('n') => number = true,
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
    let mut lineno = 0usize;
    if files.is_empty() {
        if let Ok(mut r) = io::stdin()
            && let Err(Failure::Read(e) | Failure::Write(e)) =
                cat_stream(&mut r, &mut w, number, &mut lineno).await
        {
            return Err(e.into());
        }
        return Ok(0);
    }
    // Like GNU, report each file that can't be opened or read and go on
    // with the rest, but stop at the first failed write to stdout. Stderr is
    // taken once, on the first failure.
    let mut ew = None;
    for path in &files {
        let result = match io::open(os, path, OpenFlags::read()).await {
            Ok(fd) => match io::take_reader(fd) {
                Ok(mut r) => cat_stream(&mut r, &mut w, number, &mut lineno).await,
                Err(e) => Err(Failure::Read(e)),
            },
            Err(e) => Err(Failure::Read(e)),
        };
        let (e, stop) = match result {
            Ok(()) => continue,
            Err(Failure::Read(e)) => (e, false),
            Err(Failure::Write(e)) => (e, true),
        };
        if ew.is_none() {
            ew = Some(io::stderr()?);
        }
        let ew = ew.as_mut().expect("stderr was just taken");
        wprintln!(ew, "strands-shell: cat: {}", e)?;
        if stop {
            break;
        }
    }
    Ok(i32::from(ew.is_some()))
}
