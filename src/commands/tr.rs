use crate::prelude::*;

const HELP: &str = "Usage: tr [OPTION] SET1 [SET2]
Translate or delete characters.

Options:
  -d    delete characters in SET1
  -s    squeeze repeated characters in SET1
  -c    complement SET1";

fn expand_set(s: &str) -> Vec<char> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // POSIX character classes [:class:]
        if i + 2 < chars.len()
            && chars[i] == '['
            && chars[i + 1] == ':'
            && let Some(end) = chars[i + 2..]
                .windows(2)
                .position(|w| w[0] == ':' && w[1] == ']')
        {
            let class: String = chars[i + 2..i + 2 + end].iter().collect();
            let range: Box<dyn Iterator<Item = char>> = match class.as_str() {
                "lower" => Box::new('a'..='z'),
                "upper" => Box::new('A'..='Z'),
                "digit" => Box::new('0'..='9'),
                "alpha" => Box::new(('a'..='z').chain('A'..='Z')),
                "alnum" => Box::new(('0'..='9').chain('a'..='z').chain('A'..='Z')),
                "space" => Box::new([' ', '\t', '\n', '\r'].into_iter()),
                "blank" => Box::new([' ', '\t'].into_iter()),
                _ => {
                    out.push(chars[i]);
                    i += 1;
                    continue;
                }
            };
            out.extend(range);
            i += 2 + end + 2; // skip [:class:]
            continue;
        }
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let start = chars[i] as u32;
            let end = chars[i + 2] as u32;
            for c in start..=end {
                if let Some(ch) = char::from_u32(c) {
                    out.push(ch);
                }
            }
            i += 3;
        } else if chars[i] == '\\' && i + 1 < chars.len() {
            out.push(match chars[i + 1] {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                c => c,
            });
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[command("tr")]
async fn cmd_tr(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let _ = os;
    let mut parser = lexopt::Parser::from_args(args);
    let mut delete = false;
    let mut squeeze = false;
    let mut complement = false;
    let mut sets = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') => delete = true,
            Short('s') => squeeze = true,
            Short('c') | Short('C') => complement = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => sets.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if sets.is_empty() {
        return Err("tr: missing operand".into());
    }

    let set1 = expand_set(&sets[0]);
    let set2 = if sets.len() > 1 {
        expand_set(&sets[1])
    } else {
        Vec::new()
    };

    let set1_contains = |c: char| -> bool {
        let found = set1.contains(&c);
        if complement { !found } else { found }
    };

    let mut r = io::stdin()?;
    let mut w = io::stdout()?;
    let mut buf = [0u8; 8192];
    let mut last_out: Option<char> = None;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buf[..n]);
        for c in text.chars() {
            if delete {
                if !set1_contains(c) {
                    if squeeze && set2.contains(&c) && last_out == Some(c) {
                        continue;
                    }
                    wprint!(w, "{}", c)?;
                    last_out = Some(c);
                }
            } else if !set2.is_empty() {
                let out = if set1_contains(c) {
                    let idx = if complement {
                        0 // complement translate: map all non-set1 chars
                    } else {
                        set1.iter().position(|&x| x == c).unwrap_or(0)
                    };
                    *set2.get(idx).or(set2.last()).unwrap_or(&c)
                } else {
                    c
                };
                if squeeze && last_out == Some(out) {
                    continue;
                }
                wprint!(w, "{}", out)?;
                last_out = Some(out);
            } else if squeeze {
                if set1_contains(c) && last_out == Some(c) {
                    continue;
                }
                wprint!(w, "{}", c)?;
                last_out = Some(c);
            } else {
                wprint!(w, "{}", c)?;
                last_out = Some(c);
            }
        }
    }
    Ok(0)
}
