use crate::prelude::*;

const HELP: &str = "Usage: wc [-lwc] [FILE]...
Print newline, word, and byte counts.

Options:
  -l    print line count
  -w    print word count
  -c    print byte count";

struct Counts {
    lines: usize,
    words: usize,
    bytes: usize,
}

async fn count_stream<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Counts> {
    let mut c = Counts {
        lines: 0,
        words: 0,
        bytes: 0,
    };
    let mut buf = [0u8; 8192];
    let mut in_word = false;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        c.bytes += n;
        for &b in &buf[..n] {
            if b == b'\n' {
                c.lines += 1;
            }
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                in_word = false;
            } else if !in_word {
                in_word = true;
                c.words += 1;
            }
        }
    }
    Ok(c)
}

#[command("wc")]
async fn cmd_wc(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut opt_l = false;
    let mut opt_w = false;
    let mut opt_c = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('l') => opt_l = true,
            Short('w') => opt_w = true,
            Short('c') => opt_c = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if !opt_l && !opt_w && !opt_c {
        opt_l = true;
        opt_w = true;
        opt_c = true;
    }

    let mut w = io::stdout()?;
    let mut total = Counts {
        lines: 0,
        words: 0,
        bytes: 0,
    };

    if files.is_empty() {
        let mut r = io::stdin()?;
        let c = count_stream(&mut r).await?;
        if opt_l {
            wprint!(w, "{:>7}", c.lines)?;
        }
        if opt_w {
            wprint!(w, "{:>7}", c.words)?;
        }
        if opt_c {
            wprint!(w, "{:>7}", c.bytes)?;
        }
        wprintln!(w)?;
        return Ok(0);
    }

    for path in &files {
        let fd = io::open(os, path, OpenFlags::read()).await?;
        let mut r = io::take_reader(fd)?;
        let c = count_stream(&mut r).await?;
        total.lines += c.lines;
        total.words += c.words;
        total.bytes += c.bytes;
        if opt_l {
            wprint!(w, "{:>7}", c.lines)?;
        }
        if opt_w {
            wprint!(w, "{:>7}", c.words)?;
        }
        if opt_c {
            wprint!(w, "{:>7}", c.bytes)?;
        }
        wprintln!(w, " {}", path)?;
    }
    if files.len() > 1 {
        if opt_l {
            wprint!(w, "{:>7}", total.lines)?;
        }
        if opt_w {
            wprint!(w, "{:>7}", total.words)?;
        }
        if opt_c {
            wprint!(w, "{:>7}", total.bytes)?;
        }
        wprintln!(w, " total")?;
    }
    Ok(0)
}
