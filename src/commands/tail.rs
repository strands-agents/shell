use crate::prelude::*;
use std::collections::VecDeque;

const HELP: &str = "Usage: tail [-n LINES] [FILE]
Output the last part of files.

Options:
  -n LINES    number of lines (default: 10); +N means starting from line N";

#[command("tail")]
async fn cmd_tail(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut count: usize = 10;
    let mut from_start = false;
    let mut file = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('n') | Long("lines") => {
                let val = parser.value()?.string()?;
                if let Some(rest) = val.strip_prefix('+') {
                    from_start = true;
                    count = rest.parse().unwrap_or(1);
                } else {
                    count = val.parse().unwrap_or(10);
                }
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
    let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if let Some(path) = &file {
        let fd = io::open(os, path, OpenFlags::read()).await?;
        Box::new(io::take_reader(fd)?)
    } else {
        Box::new(io::stdin()?)
    };
    let mut reader = BufReader::new(reader);
    let mut w = io::stdout()?;

    if from_start {
        // Skip first count-1 lines, print the rest
        let mut line = String::new();
        for _ in 1..count {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                return Ok(0);
            }
        }
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                break;
            }
            w.write_all(line.as_bytes()).await?;
        }
    } else {
        // Keep last N lines in a ring buffer
        let mut ring: VecDeque<String> = VecDeque::with_capacity(count + 1);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                break;
            }
            ring.push_back(line.clone());
            if ring.len() > count {
                ring.pop_front();
            }
        }
        for l in &ring {
            w.write_all(l.as_bytes()).await?;
        }
    }
    Ok(0)
}
