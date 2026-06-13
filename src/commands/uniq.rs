use crate::prelude::*;

const HELP: &str = "Usage: uniq [OPTIONS] [INPUT [OUTPUT]]
Filter adjacent matching lines.

Options:
  -c    prefix lines by the number of occurrences
  -d    only print duplicate lines
  -u    only print unique lines
  -i    ignore differences in case
  -f N  avoid comparing the first N fields
  -s N  avoid comparing the first N characters";

async fn flush_line(
    w: &mut crate::os::FdWriter,
    prev: &str,
    cnt: usize,
    count: bool,
    only_dup: bool,
    only_uniq: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if prev.is_empty() {
        return Ok(());
    }
    let show = (!only_dup && !only_uniq) || (only_dup && cnt > 1) || (only_uniq && cnt == 1);
    if show {
        if count {
            wprintln!(w, "{:>7} {}", cnt, prev)?;
        } else {
            wprintln!(w, "{}", prev)?;
        }
    }
    Ok(())
}

#[command("uniq")]
async fn cmd_uniq(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut count = false;
    let mut only_dup = false;
    let mut only_uniq = false;
    let mut ignore_case = false;
    let mut skip_fields: usize = 0;
    let mut skip_chars: usize = 0;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('c') => count = true,
            Short('d') => only_dup = true,
            Short('u') => only_uniq = true,
            Short('i') => ignore_case = true,
            Short('f') => skip_fields = parser.value()?.parse()?,
            Short('s') => skip_chars = parser.value()?.parse()?,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }

    let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if files.is_empty() {
        Box::new(io::stdin()?)
    } else {
        let fd = io::open(os, &files[0], OpenFlags::read()).await?;
        Box::new(io::take_reader(fd)?)
    };
    let mut reader = BufReader::new(reader);
    let mut w = io::stdout()?;

    let key = |line: &str| -> String {
        let mut s = line;
        if skip_fields > 0 {
            let mut remaining = s;
            for _ in 0..skip_fields {
                remaining = remaining.trim_start();
                match remaining.find(char::is_whitespace) {
                    Some(i) => remaining = &remaining[i..],
                    None => {
                        remaining = "";
                        break;
                    }
                }
            }
            s = remaining;
        }
        // `-s N` skips the first N *characters*; index by char boundary so a
        // multibyte char (e.g. a leading `é`) can't panic a byte slice.
        let s = if skip_chars > 0 {
            match s.char_indices().nth(skip_chars) {
                Some((byte_idx, _)) => &s[byte_idx..],
                None => "",
            }
        } else {
            s
        };
        if ignore_case {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };

    let mut line = String::new();
    let mut prev_line = String::new();
    let mut prev_key = String::new();
    let mut cnt: usize = 0;

    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let l = if line.ends_with('\n') {
            &line[..line.len() - 1]
        } else {
            &line[..]
        };
        let k = key(l);
        if cnt == 0 || k != prev_key {
            flush_line(&mut w, &prev_line, cnt, count, only_dup, only_uniq).await?;
            prev_line = l.to_string();
            prev_key = k;
            cnt = 1;
        } else {
            cnt += 1;
        }
    }
    flush_line(&mut w, &prev_line, cnt, count, only_dup, only_uniq).await?;
    Ok(0)
}
