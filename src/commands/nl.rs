use crate::prelude::*;

const HELP: &str = "Usage: nl [OPTION]... [FILE]...
Write each FILE to standard output, with line numbers added.
With no FILE, or when FILE is -, read standard input.

Options:
  -b, --body-numbering=STYLE      use STYLE for numbering body lines
  -n, --number-format=FORMAT      insert line numbers according to FORMAT
  -s, --number-separator=STRING   add STRING after (possible) line number
  -w, --number-width=NUMBER       use NUMBER columns for line numbers

STYLE is one of:
  a   number all lines
  t   number only nonempty lines (default)
  n   number no lines

FORMAT is one of:
  ln  left justified, no leading zeros
  rn  right justified, no leading zeros (default)
  rz  right justified, leading zeros";

#[derive(Clone, Copy, PartialEq)]
enum Style {
    All,
    NonEmpty,
    None,
}

#[derive(Clone, Copy)]
enum Format {
    Left,
    Right,
    RightZero,
}

struct Options {
    style: Style,
    format: Format,
    width: usize,
    separator: String,
}

impl Options {
    /// The prefix for a numbered line.
    fn numbered(&self, n: u64) -> String {
        let (w, sep) = (self.width, &self.separator);
        match self.format {
            Format::Left => format!("{n:<w$}{sep}"),
            Format::Right => format!("{n:>w$}{sep}"),
            Format::RightZero => format!("{n:0>w$}{sep}"),
        }
    }
}

async fn nl_stream(
    reader: impl tokio::io::AsyncRead + Unpin,
    opts: &Options,
    number: &mut u64,
    w: &mut crate::os::FdWriter,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(reader);
    // Unnumbered lines are indented by the width of the number and separator.
    let blank = " ".repeat(opts.width + opts.separator.len());
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') {
            line.push(b'\n');
        }
        let numbered = match opts.style {
            Style::All => true,
            Style::NonEmpty => line.len() > 1,
            Style::None => false,
        };
        let mut out = if numbered {
            *number += 1;
            opts.numbered(*number).into_bytes()
        } else {
            blank.clone().into_bytes()
        };
        out.extend_from_slice(&line);
        w.write_all(&out).await?;
    }
}

#[command("nl")]
async fn cmd_nl(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut opts = Options {
        style: Style::NonEmpty,
        format: Format::Right,
        width: 6,
        separator: "\t".to_string(),
    };
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('b') | Long("body-numbering") => {
                let value = parser.value()?.string()?;
                opts.style = match value.as_str() {
                    "a" => Style::All,
                    "t" => Style::NonEmpty,
                    "n" => Style::None,
                    _ => return Err(format!("invalid body numbering style: '{value}'").into()),
                };
            }
            Short('n') | Long("number-format") => {
                let value = parser.value()?.string()?;
                opts.format = match value.as_str() {
                    "ln" => Format::Left,
                    "rn" => Format::Right,
                    "rz" => Format::RightZero,
                    _ => return Err(format!("invalid line numbering format: '{value}'").into()),
                };
            }
            Short('s') | Long("number-separator") => opts.separator = parser.value()?.string()?,
            Short('w') | Long("number-width") => {
                let value = parser.value()?.string()?;
                opts.width = value
                    .parse()
                    .ok()
                    .filter(|&w| w > 0)
                    .ok_or_else(|| format!("invalid line number field width: '{value}'"))?;
            }
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
    let mut number = 0;
    let mut status = 0;
    // Stderr is taken once, on the first error, so later errors are reported too.
    let mut stderr = None;
    for path in &files {
        let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if path == "-" {
            Box::new(io::stdin()?)
        } else {
            match io::open(os, path, OpenFlags::read()).await {
                Ok(fd) => Box::new(io::take_reader(fd)?),
                Err(e) => {
                    let ew = match &mut stderr {
                        Some(ew) => ew,
                        None => stderr.insert(io::stderr()?),
                    };
                    wprintln!(ew, "nl: {path}: {e}")?;
                    status = 1;
                    continue;
                }
            }
        };
        nl_stream(reader, &opts, &mut number, &mut w).await?;
    }
    Ok(status)
}
