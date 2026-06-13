use crate::prelude::*;

const HELP: &str = "Usage: chmod MODE FILE...
Change file mode bits.

MODE is an octal number (e.g. 755) or symbolic (e.g. +x, u+rw, go-w).";

fn parse_symbolic_mode(
    mode_str: &str,
    current: u32,
) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
    let mut result = current & 0o7777;
    for clause in mode_str.split(',') {
        let mut chars = clause.chars().peekable();
        // Parse who: u, g, o, a
        let mut who: u32 = 0;
        while let Some(&c) = chars.peek() {
            match c {
                'u' => {
                    who |= 0o700;
                    chars.next();
                }
                'g' => {
                    who |= 0o070;
                    chars.next();
                }
                'o' => {
                    who |= 0o007;
                    chars.next();
                }
                'a' => {
                    who |= 0o777;
                    chars.next();
                }
                _ => break,
            }
        }
        if who == 0 {
            who = 0o777;
        }
        // Parse op: +, -, =
        let op = chars.next().ok_or("chmod: invalid mode")?;
        if !matches!(op, '+' | '-' | '=') {
            return Err(format!("chmod: invalid operator '{}'", op).into());
        }
        // Parse perms: r, w, x
        let mut perms: u32 = 0;
        for c in chars {
            match c {
                'r' => perms |= 0o444,
                'w' => perms |= 0o222,
                'x' => perms |= 0o111,
                _ => return Err(format!("chmod: invalid permission '{}'", c).into()),
            }
        }
        let bits = perms & who;
        match op {
            '+' => result |= bits,
            '-' => result &= !bits,
            '=' => result = (result & !who) | bits,
            _ => unreachable!(),
        }
    }
    Ok(result)
}

#[command("chmod")]
async fn cmd_chmod(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut mode_str = None;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => {
                let s = val.string()?;
                if mode_str.is_none() {
                    mode_str = Some(s);
                } else {
                    files.push(s);
                }
            }
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mode_str = mode_str.ok_or("chmod: missing operand")?;
    if files.is_empty() {
        return Err("chmod: missing file operand".into());
    }

    for path in &files {
        let mode = if mode_str.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            u32::from_str_radix(&mode_str, 8)?
        } else {
            let st = io::stat(os, path).await;
            parse_symbolic_mode(&mode_str, st.mode)?
        };
        io::set_permissions(os, path, mode).await?;
    }
    Ok(0)
}
