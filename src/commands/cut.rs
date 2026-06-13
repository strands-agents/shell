use crate::prelude::*;

const HELP: &str = "Usage: cut OPTION [FILE]...
Remove sections from each line.

Options:
  -d DELIM   use DELIM instead of TAB
  -f FIELDS  select only these fields (1-based, comma/dash separated)
  -c CHARS   select only these characters (1-based, comma/dash separated)
  -s         do not print lines not containing delimiters (with -f)";

fn parse_ranges(spec: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            let start = a.parse::<usize>().unwrap_or(1);
            let end = if b.is_empty() {
                usize::MAX
            } else {
                b.parse().unwrap_or(usize::MAX)
            };
            ranges.push((start, end));
        } else if let Ok(n) = part.parse::<usize>() {
            ranges.push((n, n));
        }
    }
    ranges
}

fn in_ranges(pos: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|&(s, e)| pos >= s && pos <= e)
}

#[command("cut")]
async fn cmd_cut(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut delim = '\t';
    let mut field_spec = String::new();
    let mut char_spec = String::new();
    let mut suppress = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') => {
                let v = parser.value()?.string()?;
                delim = v.chars().next().unwrap_or('\t');
            }
            Short('f') => field_spec = parser.value()?.string()?,
            Short('c') => char_spec = parser.value()?.string()?,
            Short('s') => suppress = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if field_spec.is_empty() && char_spec.is_empty() {
        return Err("cut: you must specify a list of bytes, characters, or fields".into());
    }

    let ranges = parse_ranges(if !field_spec.is_empty() {
        &field_spec
    } else {
        &char_spec
    });
    let by_field = !field_spec.is_empty();

    let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if files.is_empty() {
        Box::new(io::stdin()?)
    } else {
        let fd = io::open(os, &files[0], OpenFlags::read()).await?;
        Box::new(io::take_reader(fd)?)
    };
    let mut reader = BufReader::new(reader);
    let mut w = io::stdout()?;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let l = line.trim_end_matches('\n');
        if by_field {
            let fields: Vec<&str> = l.split(delim).collect();
            if fields.len() == 1 && suppress {
                continue;
            }
            let selected: Vec<&str> = fields
                .iter()
                .enumerate()
                .filter(|(i, _)| in_ranges(i + 1, &ranges))
                .map(|(_, s)| *s)
                .collect();
            wprintln!(w, "{}", selected.join(&delim.to_string()))?;
        } else {
            let chars: Vec<char> = l.chars().collect();
            let selected: String = chars
                .iter()
                .enumerate()
                .filter(|(i, _)| in_ranges(i + 1, &ranges))
                .map(|(_, c)| *c)
                .collect();
            wprintln!(w, "{}", selected)?;
        }
    }
    Ok(0)
}
