use crate::prelude::*;
use tokio::io::AsyncBufRead;

const HELP: &str = "Usage: paste [OPTION]... [FILE]...
Write lines consisting of the sequentially corresponding lines from
each FILE, separated by TABs, to standard output.
With no FILE, or when FILE is -, read standard input.

Options:
  -d, --delimiters=LIST   reuse characters from LIST instead of TABs
  -s, --serial            paste one file at a time instead of in parallel";

type Reader = BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>;

/// Expands the escapes GNU paste accepts in a delimiter list; `\0` is an empty delimiter.
fn parse_delimiters(list: &str) -> Result<Vec<String>, String> {
    let mut delims = Vec::new();
    let mut chars = list.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            delims.push(c.to_string());
            continue;
        }
        let escaped = match chars.next() {
            Some('0') => "",
            Some('b') => "\x08",
            Some('f') => "\x0c",
            Some('n') => "\n",
            Some('r') => "\r",
            Some('t') => "\t",
            Some('v') => "\x0b",
            Some(other) => {
                delims.push(other.to_string());
                continue;
            }
            None => {
                return Err(format!(
                    "delimiter list ends with an unescaped backslash: {list}"
                ));
            }
        };
        delims.push(escaped.to_string());
    }
    Ok(delims)
}

/// Reads one line without its newline, or None at end of input.
async fn next_line(reader: &mut (impl AsyncBufRead + Unpin)) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    if reader.read_until(b'\n', &mut line).await? == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    Ok(Some(line))
}

/// An input: standard input (shared by every `-`), a file, or one that is done.
enum Input {
    Stdin,
    File(Reader),
    Done,
}

#[command("paste")]
async fn cmd_paste(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut delims = vec!["\t".to_string()];
    let mut serial = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') | Long("delimiters") => {
                delims = parse_delimiters(&parser.value()?.string()?)?;
            }
            Short('s') | Long("serial") => serial = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if delims.is_empty() {
        delims.push(String::new());
    }
    if files.is_empty() {
        files.push("-".to_string());
    }

    let mut stdin: Option<Reader> = None;
    let mut inputs = Vec::with_capacity(files.len());
    for path in &files {
        if path == "-" {
            if stdin.is_none() {
                stdin = Some(BufReader::new(Box::new(io::stdin()?)));
            }
            inputs.push(Input::Stdin);
            continue;
        }
        let fd = io::open(os, path, OpenFlags::read()).await?;
        inputs.push(Input::File(BufReader::new(Box::new(io::take_reader(fd)?))));
    }

    let mut w = io::stdout()?;
    if serial {
        for input in &mut inputs {
            let reader = match input {
                Input::Stdin => stdin.as_mut().expect("stdin is open for -"),
                Input::File(reader) => reader,
                Input::Done => continue,
            };
            let mut out = Vec::new();
            let mut joins = 0;
            while let Some(line) = next_line(reader).await? {
                if joins > 0 {
                    out.extend_from_slice(delims[(joins - 1) % delims.len()].as_bytes());
                }
                out.extend_from_slice(&line);
                joins += 1;
            }
            out.push(b'\n');
            w.write_all(&out).await?;
        }
        return Ok(0);
    }

    loop {
        let mut row = Vec::new();
        let mut any = false;
        for (i, input) in inputs.iter_mut().enumerate() {
            let line = match input {
                Input::Stdin => next_line(stdin.as_mut().expect("stdin is open for -")).await?,
                Input::File(reader) => next_line(reader).await?,
                Input::Done => None,
            };
            match line {
                Some(line) => {
                    any = true;
                    row.extend_from_slice(&line);
                }
                None => *input = Input::Done,
            }
            if i + 1 < files.len() {
                row.extend_from_slice(delims[i % delims.len()].as_bytes());
            }
        }
        if !any {
            return Ok(0);
        }
        row.push(b'\n');
        w.write_all(&row).await?;
    }
}
