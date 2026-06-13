use crate::prelude::*;

const HELP: &str = "Usage: head [-n LINES] [FILE]
Output the first part of files.

Options:
  -n, --lines LINES   number of lines to show (default: 10)";

#[command("head")]
async fn cmd_head(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut n: usize = 10;
    let mut file = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('n') | Long("lines") => n = parser.value()?.parse()?,
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
