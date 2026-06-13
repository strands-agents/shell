use crate::prelude::*;

const HELP: &str = "Usage: sed [OPTIONS] [SCRIPT] [FILE...]
Stream editor for filtering and transforming text.

Options:
  -e SCRIPT  add SCRIPT to the commands to be executed
  -n         suppress automatic printing of pattern space
  -i[SUFFIX] edit files in place (optionally creating backup)";

/// A parsed address.
#[derive(Clone)]
enum Addr {
    Line(usize),
    Last,
    Regex(String, bool), // (pattern, case_insensitive)
}

/// A parsed sed command.
#[derive(Clone)]
struct Cmd {
    addr1: Option<Addr>,
    addr2: Option<Addr>,
    negated: bool,
    op: Op,
}

#[derive(Clone)]
enum Op {
    Sub {
        pattern: String,
        replacement: String,
        global: bool,
        icase: bool,
        print: bool,
    },
    Delete,
    Print,
    Quit,
    Append(String),
    Insert(String),
    Change(String),
    TranslateY(Vec<char>, Vec<char>),
    HoldAppend,        // H — append pattern to hold
    HoldReplace,       // h — copy pattern to hold
    GetAppend,         // G — append hold to pattern
    GetReplace,        // g — copy hold to pattern (not the 'g' flag!)
    Exchange,          // x — swap pattern and hold
    WriteFile(String), // w FILE
}

fn parse_addr(s: &str, pos: &mut usize) -> Option<Addr> {
    let chars: Vec<char> = s.chars().collect();
    if *pos >= chars.len() {
        return None;
    }
    if chars[*pos] == '$' {
        *pos += 1;
        return Some(Addr::Last);
    }
    if chars[*pos].is_ascii_digit() {
        let start = *pos;
        while *pos < chars.len() && chars[*pos].is_ascii_digit() {
            *pos += 1;
        }
        let n: usize = s[start..*pos].parse().unwrap_or(0);
        return Some(Addr::Line(n));
    }
    if chars[*pos] == '/' {
        *pos += 1;
        let mut pat = String::new();
        while *pos < chars.len() && chars[*pos] != '/' {
            if chars[*pos] == '\\' && *pos + 1 < chars.len() {
                *pos += 1;
                if chars[*pos] != '/' {
                    pat.push('\\');
                }
                pat.push(chars[*pos]);
            } else {
                pat.push(chars[*pos]);
            }
            *pos += 1;
        }
        if *pos < chars.len() {
            *pos += 1;
        } // skip closing /
        return Some(Addr::Regex(pat, false));
    }
    None
}

fn parse_sub(s: &str, pos: &mut usize) -> Option<Op> {
    let chars: Vec<char> = s.chars().collect();
    if *pos >= chars.len() {
        return None;
    }
    let delim = chars[*pos];
    *pos += 1;
    // Read pattern
    let mut pattern = String::new();
    while *pos < chars.len() && chars[*pos] != delim {
        if chars[*pos] == '\\' && *pos + 1 < chars.len() {
            *pos += 1;
            if chars[*pos] != delim {
                pattern.push('\\');
            }
            pattern.push(chars[*pos]);
        } else {
            pattern.push(chars[*pos]);
        }
        *pos += 1;
    }
    if *pos < chars.len() {
        *pos += 1;
    } // skip delim
    // Read replacement
    let mut replacement = String::new();
    while *pos < chars.len() && chars[*pos] != delim {
        if chars[*pos] == '\\' && *pos + 1 < chars.len() {
            *pos += 1;
            replacement.push('\\');
            replacement.push(chars[*pos]);
        } else {
            replacement.push(chars[*pos]);
        }
        *pos += 1;
    }
    if *pos < chars.len() {
        *pos += 1;
    } // skip delim
    // Read flags
    let mut global = false;
    let mut icase = false;
    let mut print = false;
    while *pos < chars.len() && chars[*pos] != ';' && chars[*pos] != '}' && chars[*pos] != '\n' {
        match chars[*pos] {
            'g' => global = true,
            'i' | 'I' => icase = true,
            'p' => print = true,
            _ => {}
        }
        *pos += 1;
    }
    Some(Op::Sub {
        pattern,
        replacement,
        global,
        icase,
        print,
    })
}

fn parse_text_arg(s: &str, pos: &mut usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    // skip optional whitespace and backslash-newline
    while *pos < chars.len() && (chars[*pos] == ' ' || chars[*pos] == '\t' || chars[*pos] == '\\') {
        if chars[*pos] == '\\' && *pos + 1 < chars.len() && chars[*pos + 1] == '\n' {
            *pos += 2;
        } else if chars[*pos] == '\\' {
            *pos += 1;
            break;
        } else {
            *pos += 1;
        }
    }
    let start = *pos;
    *pos = chars.len();
    s[start..].to_string()
}

fn parse_script(script: &str) -> Result<Vec<Cmd>, String> {
    let mut cmds = Vec::new();
    let chars: Vec<char> = script.chars().collect();
    let mut pos = 0;
    // Stack of (addr1, addr2, negated) for nested { } groups
    let mut group_stack: Vec<(Option<Addr>, Option<Addr>, bool)> = Vec::new();

    while pos < chars.len() {
        // Skip whitespace, semicolons, newlines
        while pos < chars.len()
            && (chars[pos] == ' ' || chars[pos] == '\t' || chars[pos] == '\n' || chars[pos] == ';')
        {
            pos += 1;
        }
        if pos >= chars.len() {
            break;
        }

        if chars[pos] == '}' {
            group_stack.pop();
            pos += 1;
            continue;
        }

        let addr1 = parse_addr(script, &mut pos);
        // Skip comma for range
        let addr2 = if pos < chars.len() && chars[pos] == ',' {
            pos += 1;
            parse_addr(script, &mut pos)
        } else {
            None
        };

        // Skip whitespace
        while pos < chars.len() && (chars[pos] == ' ' || chars[pos] == '\t') {
            pos += 1;
        }
        if pos >= chars.len() {
            break;
        }

        let negated = if chars[pos] == '!' {
            pos += 1;
            true
        } else {
            false
        };
        while pos < chars.len() && (chars[pos] == ' ' || chars[pos] == '\t') {
            pos += 1;
        }
        if pos >= chars.len() {
            break;
        }

        let op_char = chars[pos];
        pos += 1;

        if op_char == '{' {
            // Push group address onto stack — commands inside inherit it
            group_stack.push((addr1, addr2, negated));
            continue;
        }

        // Determine effective address: use this command's address, or inherit from group
        let (eff_addr1, eff_addr2, eff_negated) = if addr1.is_some() || negated {
            (addr1, addr2, negated)
        } else if let Some((ga1, ga2, gn)) = group_stack.last() {
            (ga1.clone(), ga2.clone(), *gn)
        } else {
            (addr1, addr2, negated)
        };

        let op = match op_char {
            's' => parse_sub(script, &mut pos).ok_or("sed: invalid s command")?,
            'd' => Op::Delete,
            'p' => Op::Print,
            'q' => Op::Quit,
            'a' => Op::Append(parse_text_arg(script, &mut pos)),
            'i' => {
                // Disambiguate: 'i' as insert vs 'i' flag after s///
                // If we're here, it's the insert command
                Op::Insert(parse_text_arg(script, &mut pos))
            }
            'c' => Op::Change(parse_text_arg(script, &mut pos)),
            'y' => {
                // y/src/dst/
                if pos >= chars.len() {
                    return Err("sed: invalid y command".into());
                }
                let delim = chars[pos];
                pos += 1;
                let mut src = Vec::new();
                while pos < chars.len() && chars[pos] != delim {
                    src.push(chars[pos]);
                    pos += 1;
                }
                if pos < chars.len() {
                    pos += 1;
                }
                let mut dst = Vec::new();
                while pos < chars.len() && chars[pos] != delim {
                    dst.push(chars[pos]);
                    pos += 1;
                }
                if pos < chars.len() {
                    pos += 1;
                }
                if src.len() != dst.len() {
                    return Err("sed: y: transform strings are not the same length".into());
                }
                Op::TranslateY(src, dst)
            }
            'H' => Op::HoldAppend,
            'h' => Op::HoldReplace,
            'G' => Op::GetAppend,
            'g' if pos < chars.len() && chars[pos] != '/' => Op::GetReplace,
            'x' => Op::Exchange,
            'w' => {
                // w FILE — read filename
                while pos < chars.len() && chars[pos] == ' ' {
                    pos += 1;
                }
                let start = pos;
                while pos < chars.len() && chars[pos] != ';' && chars[pos] != '\n' {
                    pos += 1;
                }
                let fname = script[start..pos].trim().to_string();
                Op::WriteFile(fname)
            }
            '{' => continue, // handled above
            _ => return Err(format!("sed: unknown command: '{op_char}'")),
        };

        cmds.push(Cmd {
            addr1: eff_addr1,
            addr2: eff_addr2,
            negated: eff_negated,
            op,
        });
    }
    Ok(cmds)
}

fn addr_matches(addr: &Addr, lineno: usize, line: &str, is_last: bool) -> bool {
    match addr {
        Addr::Line(n) => lineno == *n,
        Addr::Last => is_last,
        Addr::Regex(pat, icase) => {
            let flags = if *icase { "(?i)" } else { "" };
            regex::Regex::new(&format!("{flags}{pat}"))
                .map(|re| re.is_match(line))
                .unwrap_or(false)
        }
    }
}

/// Convert BRE (Basic Regular Expression) escapes to ERE for the regex crate.
/// BRE uses \( \) \{ \} for groups/repetition; ERE uses ( ) { }.
fn bre_to_ere(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len());
    let chars: Vec<char> = pat.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                '(' | ')' | '{' | '}' => {
                    out.push(chars[i + 1]);
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    out.push(chars[i + 1]);
                    i += 2;
                }
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn apply_sub(
    line: &str,
    pattern: &str,
    replacement: &str,
    global: bool,
    icase: bool,
) -> Option<String> {
    let flags = if icase { "(?i)" } else { "" };
    let ere_pattern = bre_to_ere(pattern);
    let re = match regex::Regex::new(&format!("{flags}{ere_pattern}")) {
        Ok(r) => r,
        Err(_) => return None,
    };
    if !re.is_match(line) {
        return None;
    }
    // Process replacement: handle \1-\9 and & (whole match)
    let result = if global {
        re.replace_all(line, |caps: &regex::Captures| {
            expand_replacement(replacement, caps)
        })
    } else {
        re.replace(line, |caps: &regex::Captures| {
            expand_replacement(replacement, caps)
        })
    };
    Some(result.into_owned())
}

fn expand_replacement(replacement: &str, caps: &regex::Captures) -> String {
    let mut out = String::new();
    let chars: Vec<char> = replacement.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next.is_ascii_digit() {
                let idx = (next as u8 - b'0') as usize;
                if let Some(m) = caps.get(idx) {
                    out.push_str(m.as_str());
                }
                i += 2;
                continue;
            }
            match next {
                'n' => {
                    out.push('\n');
                    i += 2;
                }
                't' => {
                    out.push('\t');
                    i += 2;
                }
                '\\' => {
                    out.push('\\');
                    i += 2;
                }
                _ => {
                    out.push(next);
                    i += 2;
                }
            }
        } else if chars[i] == '&' {
            if let Some(m) = caps.get(0) {
                out.push_str(m.as_str());
            }
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[command("sed")]
async fn cmd_sed(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut scripts: Vec<String> = Vec::new();
    let mut suppress = false;
    let mut in_place: Option<Option<String>> = None; // None = not in-place, Some(None) = -i, Some(Some(suffix)) = -i.suffix
    let mut files: Vec<String> = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Short('n') => suppress = true,
            Short('e') => scripts.push(parser.value()?.to_string_lossy().into_owned()),
            Short('i') => {
                // -i may have an optional suffix attached or as next arg
                let val = parser.optional_value();
                in_place = Some(val.map(|v| v.to_string_lossy().into_owned()));
            }
            Value(v) if scripts.is_empty() && files.is_empty() => {
                scripts.push(v.to_string_lossy().into_owned());
            }
            Value(v) => files.push(v.to_string_lossy().into_owned()),
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{HELP}")?;
                return Ok(0);
            }
            _ => {}
        }
    }

    if scripts.is_empty() {
        let mut w = io::stderr()?;
        wprintln!(w, "sed: no script specified")?;
        return Ok(1);
    }

    let combined = scripts.join("\n");
    let cmds = match parse_script(&combined) {
        Ok(c) => c,
        Err(e) => {
            let mut w = io::stderr()?;
            wprintln!(w, "{e}")?;
            return Ok(1);
        }
    };

    if files.is_empty() && in_place.is_none() {
        let r = io::stdin()?;
        let mut w = io::stdout()?;
        process_stream(r, &mut w, &cmds, suppress).await?;
    } else if files.is_empty() {
        // -i with no files is a no-op
    } else if let Some(suffix) = in_place.as_ref() {
        for file in &files {
            // Read entire file
            let fd = io::open(os, file, OpenFlags::read()).await?;
            let mut r = io::take_reader(fd)?;
            let max_output = io::with_process(|p| p.max_output);
            let content_str = crate::os::read_to_string_limited(&mut r, max_output).await?;
            let content = content_str.into_bytes();

            // Create backup if suffix given
            if let Some(suf) = suffix {
                let backup = format!("{file}{suf}");
                let bfd = io::open(os, &backup, OpenFlags::write()).await?;
                let mut bw = io::take_writer(bfd)?;
                bw.write_all(&content).await?;
            }

            // Process
            let mut output = Vec::new();
            let cursor = std::io::Cursor::new(content);
            process_stream(cursor, &mut output, &cmds, suppress).await?;

            // Write back
            let wfd = io::open(os, file, OpenFlags::write()).await?;
            let mut fw = io::take_writer(wfd)?;
            fw.write_all(&output).await?;
        }
    } else {
        let mut w = io::stdout()?;
        for file in &files {
            let fd = io::open(os, file, OpenFlags::read()).await?;
            let r = io::take_reader(fd)?;
            process_stream(r, &mut w, &cmds, suppress).await?;
        }
    }

    Ok(0)
}

async fn process_stream<R: AsyncReadExt + Unpin, W: AsyncWriteExt + Unpin>(
    reader: R,
    w: &mut W,
    cmds: &[Cmd],
    suppress: bool,
) -> std::io::Result<()> {
    let mut lines_reader = BufReader::new(reader);
    let mut all_lines = Vec::new();
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = lines_reader.read_line(&mut buf).await?;
        if n == 0 {
            break;
        }
        // Strip trailing newline for processing, remember if it had one
        let has_newline = buf.ends_with('\n');
        if has_newline {
            buf.pop();
        }
        if buf.ends_with('\r') {
            buf.pop();
        }
        all_lines.push(buf.clone());
    }

    let total = all_lines.len();
    // Track range state per command
    let mut in_range: Vec<bool> = vec![false; cmds.len()];
    let mut hold = String::new();
    let mut write_files: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();

    for (idx, line) in all_lines.iter().enumerate() {
        let lineno = idx + 1;
        let is_last = lineno == total;
        let mut current = line.clone();
        let mut deleted = false;
        let mut printed_extra = false;
        let mut quit = false;
        let mut append_after: Vec<String> = Vec::new();

        for (ci, cmd) in cmds.iter().enumerate() {
            let matches = match (&cmd.addr1, &cmd.addr2) {
                (None, None) => true,
                (Some(a), None) => addr_matches(a, lineno, &current, is_last),
                (Some(a1), Some(a2)) => {
                    if !in_range[ci] {
                        if addr_matches(a1, lineno, &current, is_last) {
                            in_range[ci] = true;
                            true
                        } else {
                            false
                        }
                    } else {
                        if addr_matches(a2, lineno, &current, is_last) {
                            in_range[ci] = false;
                        }
                        true
                    }
                }
                (None, Some(_)) => true,
            };

            let active = if cmd.negated { !matches } else { matches };
            if !active {
                continue;
            }

            match &cmd.op {
                Op::Sub {
                    pattern,
                    replacement,
                    global,
                    icase,
                    print,
                } => {
                    if let Some(result) = apply_sub(&current, pattern, replacement, *global, *icase)
                    {
                        current = result;
                        if *print {
                            w.write_all(current.as_bytes()).await?;
                            w.write_all(b"\n").await?;
                            printed_extra = true;
                        }
                    }
                }
                Op::Delete => {
                    deleted = true;
                    break;
                }
                Op::Print => {
                    w.write_all(current.as_bytes()).await?;
                    w.write_all(b"\n").await?;
                    printed_extra = true;
                }
                Op::Quit => {
                    quit = true;
                    break;
                }
                Op::Append(text) => append_after.push(text.clone()),
                Op::Insert(text) => {
                    w.write_all(text.as_bytes()).await?;
                    w.write_all(b"\n").await?;
                }
                Op::Change(text) => {
                    w.write_all(text.as_bytes()).await?;
                    w.write_all(b"\n").await?;
                    deleted = true;
                    break;
                }
                Op::TranslateY(src, dst) => {
                    current = current
                        .chars()
                        .map(|c| {
                            src.iter()
                                .position(|&s| s == c)
                                .map(|i| dst[i])
                                .unwrap_or(c)
                        })
                        .collect();
                }
                Op::HoldAppend => {
                    hold.push('\n');
                    hold.push_str(&current);
                }
                Op::HoldReplace => {
                    hold = current.clone();
                }
                Op::GetAppend => {
                    current.push('\n');
                    current.push_str(&hold);
                }
                Op::GetReplace => {
                    current = hold.clone();
                }
                Op::Exchange => {
                    std::mem::swap(&mut current, &mut hold);
                }
                Op::WriteFile(fname) => {
                    let entry = write_files.entry(fname.clone()).or_default();
                    entry.extend_from_slice(current.as_bytes());
                    entry.push(b'\n');
                }
            }
        }

        if !deleted && !suppress {
            w.write_all(current.as_bytes()).await?;
            w.write_all(b"\n").await?;
        }

        for text in &append_after {
            w.write_all(text.as_bytes()).await?;
            w.write_all(b"\n").await?;
        }

        if quit {
            if !deleted && suppress && !printed_extra {
                // q with -n: print current line
            }
            break;
        }
    }

    // Write accumulated w-command output files
    for (fname, data) in &write_files {
        let os = io::kernel();
        let fd = io::open(os.as_ref(), fname, OpenFlags::write()).await?;
        let mut fw = io::take_writer(fd)?;
        fw.write_all(data).await?;
    }

    Ok(())
}
