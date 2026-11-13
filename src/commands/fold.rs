use crate::prelude::*;

const HELP: &str = "Usage: fold [OPTION]... [FILE]...
Wrap input lines in each FILE, writing to standard output.
With no FILE, or when FILE is -, read standard input.

Options:
  -b, --bytes         count bytes rather than columns
  -s, --spaces        break at spaces
  -w, --width=WIDTH   use WIDTH columns instead of 80";

const TAB_WIDTH: usize = 8;

struct Options {
    width: usize,
    bytes: bool,
    spaces: bool,
}

/// The column after `unit`, counting tabs, backspaces and carriage returns like GNU
/// unless counting bytes.
fn advance(column: usize, unit: &[u8], bytes: bool) -> usize {
    if bytes {
        return column + unit.len();
    }
    match unit {
        b"\x08" => column.saturating_sub(1),
        b"\r" => 0,
        b"\t" => column + TAB_WIDTH - column % TAB_WIDTH,
        _ => column + 1,
    }
}

/// Splits a line into the units fold counts: bytes with `-b`, else UTF-8 characters,
/// keeping each invalid byte on its own.
fn units(line: &[u8], bytes: bool) -> Vec<&[u8]> {
    if bytes {
        return line.chunks(1).collect();
    }
    let mut units = Vec::with_capacity(line.len());
    for chunk in line.utf8_chunks() {
        let valid = chunk.valid();
        units.extend(
            valid
                .char_indices()
                .map(|(i, c)| &valid.as_bytes()[i..i + c.len_utf8()]),
        );
        units.extend(chunk.invalid().chunks(1));
    }
    units
}

/// Folds one line (without its newline) into `out`, following GNU fold's algorithm.
fn fold_line(line: &[u8], opts: &Options, out: &mut Vec<u8>) {
    let mut held: Vec<&[u8]> = Vec::new();
    let mut column = 0;
    for unit in units(line, opts.bytes) {
        loop {
            let next = advance(column, unit, opts.bytes);
            if next <= opts.width {
                column = next;
                held.push(unit);
                break;
            }
            if opts.spaces
                && let Some(blank) = held.iter().rposition(|u| *u == b" " || *u == b"\t")
            {
                held.drain(..=blank).for_each(|u| out.extend_from_slice(u));
                out.push(b'\n');
                column = held.iter().fold(0, |c, u| advance(c, u, opts.bytes));
                continue;
            }
            if held.is_empty() {
                column = next;
                held.push(unit);
                break;
            }
            held.drain(..).for_each(|u| out.extend_from_slice(u));
            out.push(b'\n');
            column = 0;
        }
    }
    held.into_iter().for_each(|u| out.extend_from_slice(u));
}

async fn fold_stream(
    reader: impl tokio::io::AsyncRead + Unpin,
    opts: &Options,
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
        let mut out = Vec::with_capacity(line.len() + 1);
        fold_line(
            if newline {
                &line[..line.len() - 1]
            } else {
                &line
            },
            opts,
            &mut out,
        );
        if newline {
            out.push(b'\n');
        }
        w.write_all(&out).await?;
    }
}

/// Rewrites the obsolete `-N` width (also inside `-sN` or `-bN`) into `-wN`.
fn obsolete_width(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut takes_value = false;
    for (i, arg) in args.iter().enumerate() {
        if arg == "--" {
            out.extend(args[i..].iter().cloned());
            break;
        }
        let flags_then_digits = arg
            .strip_prefix('-')
            .filter(|_| !takes_value && !arg.starts_with("--"))
            .and_then(|body| {
                let at = body.find(|c: char| c.is_ascii_digit())?;
                let (flags, digits) = body.split_at(at);
                let valid = flags.chars().all(|c| c == 'b' || c == 's')
                    && digits.chars().all(|c| c.is_ascii_digit());
                valid.then_some((flags, digits))
            });
        takes_value = !takes_value && (arg == "-w" || arg == "--width");
        match flags_then_digits {
            Some((flags, digits)) => {
                if !flags.is_empty() {
                    out.push(format!("-{flags}"));
                }
                out.push(format!("-w{digits}"));
            }
            None => out.push(arg.clone()),
        }
    }
    out
}

#[command("fold")]
async fn cmd_fold(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(obsolete_width(args));
    let mut opts = Options {
        width: 80,
        bytes: false,
        spaces: false,
    };
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('b') | Long("bytes") => opts.bytes = true,
            Short('s') | Long("spaces") => opts.spaces = true,
            Short('w') | Long("width") => {
                let value = parser.value()?.string()?;
                opts.width = value
                    .parse()
                    .ok()
                    .filter(|&w| w > 0)
                    .ok_or_else(|| format!("invalid number of columns: '{value}'"))?;
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
                    wprintln!(ew, "fold: {path}: {e}")?;
                    status = 1;
                    continue;
                }
            }
        };
        fold_stream(reader, &opts, &mut w).await?;
    }
    Ok(status)
}
