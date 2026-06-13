use crate::prelude::*;

const HELP: &str = "Usage: tee [-a] [FILE]...
Copy stdin to stdout and each FILE.

Options:
  -a    append to files instead of overwriting";

#[command("tee")]
async fn cmd_tee(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut append = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('a') => append = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let flags = if append {
        OpenFlags::append()
    } else {
        OpenFlags::write()
    };
    let mut writers = Vec::new();
    for path in &files {
        let fd = io::open(os, path, flags).await?;
        writers.push(io::take_writer(fd)?);
    }
    let mut r = io::stdin()?;
    let mut stdout = io::stdout()?;
    let mut buf = [0u8; 8192];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        stdout.write_all(&buf[..n]).await?;
        for w in &mut writers {
            w.write_all(&buf[..n]).await?;
        }
    }
    Ok(0)
}
