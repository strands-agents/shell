use crate::prelude::*;

const HELP: &str = "Usage: comm [OPTION]... FILE1 FILE2
Compare sorted files FILE1 and FILE2 line by line.
When FILE1 or FILE2 (not both) is -, read standard input.

With no options, produce three-column output.  Column one contains
lines unique to FILE1, column two contains lines unique to FILE2,
and column three contains lines common to both files.

Options:
  -1                        suppress column 1 (lines unique to FILE1)
  -2                        suppress column 2 (lines unique to FILE2)
  -3                        suppress column 3 (lines that appear in both files)
      --check-order         check that the input is correctly sorted
      --nocheck-order       do not check that the input is correctly sorted
      --output-delimiter=STR  separate columns with STR";

type Reader = BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>;

#[derive(Clone, Copy, PartialEq)]
enum CheckOrder {
    /// Check only once an unpairable line has been seen, as GNU does by default.
    Default,
    Enabled,
    Disabled,
}

/// One input, with its current line and the two before it for order checks.
struct Input {
    reader: Reader,
    current: Option<Vec<u8>>,
    prev: Option<Vec<u8>>,
    prev2: Option<Vec<u8>>,
    warned: bool,
}

impl Input {
    async fn new(reader: Reader) -> std::io::Result<Self> {
        let mut input = Input {
            reader,
            current: None,
            prev: None,
            prev2: None,
            warned: false,
        };
        input.current = input.read().await?;
        Ok(input)
    }

    /// Reads one line without its newline, or None at end of input.
    async fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        if self.reader.read_until(b'\n', &mut line).await? == 0 {
            return Ok(None);
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        Ok(Some(line))
    }

    /// Moves to the next line, returning a pair of lines that are out of order, if any.
    ///
    /// At end of input the last two lines are checked again, since an unpairable line
    /// may have turned on checking after they were read.
    async fn step(&mut self) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
        self.prev2 = self.prev.take();
        self.prev = self.current.take();
        self.current = self.read().await?;
        let pair = match (&self.current, &self.prev, &self.prev2) {
            (Some(cur), Some(prev), _) => Some((prev.clone(), cur.clone())),
            (None, Some(prev), Some(prev2)) => Some((prev2.clone(), prev.clone())),
            _ => None,
        };
        Ok(pair)
    }
}

#[command("comm")]
async fn cmd_comm(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut show = [true; 3];
    let mut check = CheckOrder::Default;
    let mut delimiter = "\t".to_string();
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('1') => show[0] = false,
            Short('2') => show[1] = false,
            Short('3') => show[2] = false,
            Long("check-order") => check = CheckOrder::Enabled,
            Long("nocheck-order") => check = CheckOrder::Disabled,
            Long("output-delimiter") => {
                delimiter = parser.value()?.string()?;
                if delimiter.is_empty() {
                    // GNU separates columns with a NUL byte when given an empty delimiter.
                    delimiter.push('\0');
                }
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
    match files.as_slice() {
        [] => return Err("missing operand".into()),
        [only] => return Err(format!("missing operand after '{only}'").into()),
        [_, _] => {}
        [_, _, extra, ..] => return Err(format!("extra operand '{extra}'").into()),
    }
    let mut inputs = Vec::with_capacity(2);
    for path in &files {
        let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if path == "-" {
            Box::new(io::stdin()?)
        } else {
            let fd = io::open(os, path, OpenFlags::read()).await?;
            Box::new(io::take_reader(fd)?)
        };
        inputs.push(Input::new(BufReader::new(reader)).await?);
    }

    // Column prefixes: column 2 follows column 1, column 3 follows both, when shown.
    let mut prefixes = [String::new(), String::new(), String::new()];
    if show[0] {
        prefixes[1].push_str(&delimiter);
        prefixes[2].push_str(&delimiter);
    }
    if show[1] {
        prefixes[2].push_str(&delimiter);
    }

    let mut w = io::stdout()?;
    // Stderr is taken only when there is a warning, so the shell can still report an error.
    let mut stderr = None;
    let mut seen_unpairable = false;
    loop {
        let order = match (&inputs[0].current, &inputs[1].current) {
            (None, None) => break,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
        };
        let (column, line) = match order {
            std::cmp::Ordering::Equal => (2, &inputs[1].current),
            std::cmp::Ordering::Less => (0, &inputs[0].current),
            std::cmp::Ordering::Greater => (1, &inputs[1].current),
        };
        if order.is_ne() {
            seen_unpairable = true;
        }
        if show[column]
            && let Some(line) = line
        {
            let mut out = prefixes[column].clone().into_bytes();
            out.extend_from_slice(line);
            out.push(b'\n');
            w.write_all(&out).await?;
        }
        for (i, input) in inputs.iter_mut().enumerate() {
            let steps = if i == 0 { order.is_le() } else { order.is_ge() };
            if !steps {
                continue;
            }
            let pair = input.step().await?;
            let checking =
                check == CheckOrder::Enabled || (check == CheckOrder::Default && seen_unpairable);
            if let Some((prev, cur)) = pair
                && checking
                && !input.warned
                && prev > cur
            {
                input.warned = true;
                let ew = match &mut stderr {
                    Some(ew) => ew,
                    None => stderr.insert(io::stderr()?),
                };
                wprintln!(ew, "comm: file {} is not in sorted order", i + 1)?;
                if check == CheckOrder::Enabled {
                    return Ok(1);
                }
            }
        }
    }
    if inputs.iter().any(|input| input.warned) {
        let ew = match &mut stderr {
            Some(ew) => ew,
            None => stderr.insert(io::stderr()?),
        };
        wprintln!(ew, "comm: input is not in sorted order")?;
        return Ok(1);
    }
    Ok(0)
}
