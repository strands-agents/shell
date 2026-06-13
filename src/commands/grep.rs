use crate::os;
use crate::prelude::*;

const HELP: &str = "Usage: grep [OPTIONS] PATTERN [FILE...]
Search for PATTERN in each FILE or standard input.

Options:
  -i, --ignore-case         ignore case distinctions
  -v, --invert-match        select non-matching lines
  -c, --count               print only a count of matching lines
  -l, --files-with-matches  print only names of files with matches
  -L, --files-without-match print only names of files without matches
  -n, --line-number         prefix each line with line number
  -r, -R, --recursive       recursively search directories
  -w, --word-regexp         match whole words only
  -x, --line-regexp         match whole lines only
  -F, --fixed-strings       interpret PATTERN as fixed string
  -E, --extended-regexp     interpret PATTERN as extended regex (default)
  -e, --regexp PATTERN      use PATTERN for matching
  -q, --quiet, --silent     suppress all output
  -H, --with-filename       print filename with matches
  -h, --no-filename         suppress filename prefix
  -o, --only-matching       show only the matching part
  -m, --max-count NUM       stop after NUM matches per file
  -A, --after-context NUM   print NUM lines after match
  -B, --before-context NUM  print NUM lines before match
  -C, --context NUM         print NUM lines before and after match
  --include=GLOB            search only files matching GLOB
  --exclude=GLOB            skip files matching GLOB
  --exclude-dir=DIR         skip directories matching DIR";

struct Opts {
    patterns: Vec<String>,
    files: Vec<String>,
    ignore_case: bool,
    invert: bool,
    count: bool,
    list: bool,
    list_non_matching: bool,
    line_number: bool,
    recursive: bool,
    word_regexp: bool,
    line_regexp: bool,
    fixed: bool,
    quiet: bool,
    with_filename: Option<bool>,
    only_matching: bool,
    max_count: Option<u64>,
    after_context: usize,
    before_context: usize,
    include: Vec<String>,
    exclude: Vec<String>,
    exclude_dir: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Option<Opts>, Box<dyn std::error::Error + Send + Sync>> {
    let mut opts = Opts {
        patterns: Vec::new(),
        files: Vec::new(),
        ignore_case: false,
        invert: false,
        count: false,
        list: false,
        list_non_matching: false,
        line_number: false,
        recursive: false,
        word_regexp: false,
        line_regexp: false,
        fixed: false,
        quiet: false,
        with_filename: None,
        only_matching: false,
        max_count: None,
        after_context: 0,
        before_context: 0,
        include: Vec::new(),
        exclude: Vec::new(),
        exclude_dir: Vec::new(),
    };
    let mut parser = lexopt::Parser::from_args(args);
    while let Some(arg) = parser.next()? {
        match arg {
            Short('i') | Long("ignore-case") => opts.ignore_case = true,
            Short('v') | Long("invert-match") => opts.invert = true,
            Short('c') | Long("count") => opts.count = true,
            Short('l') | Long("files-with-matches") => opts.list = true,
            Short('L') | Long("files-without-match") => opts.list_non_matching = true,
            Short('n') | Long("line-number") => opts.line_number = true,
            Short('r') | Short('R') | Long("recursive") => opts.recursive = true,
            Short('w') | Long("word-regexp") => opts.word_regexp = true,
            Short('x') | Long("line-regexp") => opts.line_regexp = true,
            Short('F') | Long("fixed-strings") => opts.fixed = true,
            Short('E') | Long("extended-regexp") => {}
            Short('e') | Long("regexp") => opts.patterns.push(parser.value()?.string()?),
            Short('q') | Long("quiet") | Long("silent") => opts.quiet = true,
            Short('H') | Long("with-filename") => opts.with_filename = Some(true),
            Short('h') | Long("no-filename") => opts.with_filename = Some(false),
            Short('o') | Long("only-matching") => opts.only_matching = true,
            Short('m') | Long("max-count") => opts.max_count = Some(parser.value()?.parse()?),
            Short('A') | Long("after-context") => opts.after_context = parser.value()?.parse()?,
            Short('B') | Long("before-context") => opts.before_context = parser.value()?.parse()?,
            Short('C') | Long("context") => {
                let n: usize = parser.value()?.parse()?;
                opts.before_context = n;
                opts.after_context = n;
            }
            Long("include") => opts.include.push(parser.value()?.string()?),
            Long("exclude") => opts.exclude.push(parser.value()?.string()?),
            Long("exclude-dir") => opts.exclude_dir.push(parser.value()?.string()?),
            Long("help") => return Ok(None),
            Value(val) => {
                let s = val.string()?;
                if opts.patterns.is_empty() && opts.files.is_empty() {
                    opts.patterns.push(s);
                } else {
                    opts.files.push(s);
                }
            }
            _ => return Err(arg.unexpected().into()),
        }
    }
    if opts.patterns.is_empty() {
        return Err("grep: no pattern specified".into());
    }
    Ok(Some(opts))
}

fn build_regex(opts: &Opts) -> Result<regex::Regex, Box<dyn std::error::Error + Send + Sync>> {
    let combined = if opts.fixed {
        opts.patterns
            .iter()
            .map(|p| regex::escape(p))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        opts.patterns.join("|")
    };
    let mut pat = combined;
    if opts.word_regexp {
        pat = format!(r"\b(?:{})\b", pat);
    }
    if opts.line_regexp {
        pat = format!("^(?:{})$", pat);
    }
    let re = regex::RegexBuilder::new(&pat)
        .case_insensitive(opts.ignore_case)
        .build()?;
    Ok(re)
}

fn glob_matches(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

fn filename_from_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn file_included(path: &str, opts: &Opts) -> bool {
    let name = filename_from_path(path);
    if !opts.include.is_empty() && !opts.include.iter().any(|g| glob_matches(g, name)) {
        return false;
    }
    if opts.exclude.iter().any(|g| glob_matches(g, name)) {
        return false;
    }
    true
}

fn dir_excluded(name: &str, opts: &Opts) -> bool {
    opts.exclude_dir.iter().any(|g| glob_matches(g, name))
}

async fn collect_files_recursive(os: &dyn Kernel, path: &str, opts: &Opts, out: &mut Vec<String>) {
    let proc = io::with_process(|p| p.fork());
    let entries = match os.list_dir(&proc, path).await {
        Ok(e) => e,
        Err(_) => return,
    };
    let base = path.trim_end_matches('/');
    for entry in entries {
        if entry.is_dir {
            if dir_excluded(&entry.name, opts) {
                continue;
            }
            let child = if base == "." {
                entry.name.clone()
            } else {
                format!("{}/{}", base, entry.name)
            };
            Box::pin(collect_files_recursive(os, &child, opts, out)).await;
        } else {
            let child = if base == "." {
                entry.name.clone()
            } else {
                format!("{}/{}", base, entry.name)
            };
            if file_included(&child, opts) {
                out.push(child);
            }
        }
    }
}

async fn grep_reader<R: tokio::io::AsyncRead + Unpin + Send>(
    reader: R,
    re: &regex::Regex,
    opts: &Opts,
    prefix: &str,
    w: &mut os::FdWriter,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    let mut lineno: u64 = 0;
    let mut match_count: u64 = 0;
    let mut found = false;

    let use_context = opts.before_context > 0 || opts.after_context > 0;
    // Ring buffer for before-context
    let mut before_buf: std::collections::VecDeque<(u64, String)> =
        std::collections::VecDeque::new();
    // How many more after-context lines to print
    let mut after_remaining: usize = 0;
    // Whether we need a "--" separator before the next context group
    let mut need_sep = false;

    loop {
        line.clear();
        if buf_reader.read_line(&mut line).await? == 0 {
            break;
        }
        lineno += 1;
        let text = line.trim_end_matches('\n').trim_end_matches('\r');
        let matched = re.is_match(text) ^ opts.invert;

        if matched {
            found = true;
            match_count += 1;

            if opts.quiet || opts.list || opts.list_non_matching || opts.count {
                if let Some(max) = opts.max_count
                    && match_count >= max
                {
                    break;
                }
                continue;
            }

            if use_context {
                // Print separator between context groups
                if (opts.before_context == 0 || !before_buf.is_empty()) && need_sep {
                    wprintln!(w, "--")?;
                }
                need_sep = false;
                // Flush before-context buffer
                for (bno, btext) in before_buf.drain(..) {
                    if !prefix.is_empty() {
                        wprint!(w, "{}-", prefix)?;
                    }
                    if opts.line_number {
                        wprint!(w, "{}-", bno)?;
                    }
                    wprintln!(w, "{}", btext)?;
                }
                after_remaining = opts.after_context;
            }

            if opts.only_matching && !opts.invert {
                for m in re.find_iter(text) {
                    if !prefix.is_empty() {
                        wprint!(w, "{}:", prefix)?;
                    }
                    if opts.line_number {
                        wprint!(w, "{}:", lineno)?;
                    }
                    wprintln!(w, "{}", m.as_str())?;
                }
            } else {
                if !prefix.is_empty() {
                    wprint!(w, "{}:", prefix)?;
                }
                if opts.line_number {
                    wprint!(w, "{}:", lineno)?;
                }
                wprintln!(w, "{}", text)?;
            }

            if let Some(max) = opts.max_count
                && match_count >= max
            {
                break;
            }
        } else if use_context && found && after_remaining > 0 {
            // Print after-context line
            after_remaining -= 1;
            if !prefix.is_empty() {
                wprint!(w, "{}-", prefix)?;
            }
            if opts.line_number {
                wprint!(w, "{}-", lineno)?;
            }
            wprintln!(w, "{}", text)?;
            if after_remaining == 0 {
                need_sep = true;
            }
        } else if use_context {
            // Buffer for before-context
            if after_remaining == 0 && found && !need_sep {
                need_sep = true;
            }
            before_buf.push_back((lineno, text.to_string()));
            while before_buf.len() > opts.before_context {
                before_buf.pop_front();
            }
        }
    }

    if opts.count {
        if !prefix.is_empty() {
            wprint!(w, "{}:", prefix)?;
        }
        wprintln!(w, "{}", match_count)?;
    }
    if opts.list && found {
        wprintln!(w, "{}", prefix)?;
    }
    if opts.list_non_matching && !found {
        wprintln!(w, "{}", prefix)?;
    }

    Ok(found)
}

#[command("grep")]
async fn cmd_grep(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let opts = match parse_args(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            let mut w = io::stdout()?;
            wprintln!(w, "{}", HELP)?;
            return Ok(0);
        }
        Err(e) => return Err(e),
    };
    let re = build_regex(&opts)?;

    let mut files = opts.files.clone();
    if opts.recursive && files.is_empty() {
        files.push(".".into());
    }

    // Expand directories when -r
    if opts.recursive {
        let mut expanded = Vec::new();
        for f in &files {
            let proc = io::with_process(|p| p.fork());
            let st = os.stat(&proc, f).await;
            if st.is_dir {
                collect_files_recursive(os, f, &opts, &mut expanded).await;
            } else if file_included(f, &opts) {
                expanded.push(f.clone());
            }
        }
        files = expanded;
    }

    let multi = files.len() > 1 || opts.recursive;
    let show_name = opts.with_filename.unwrap_or(multi);

    let mut w = io::stdout()?;
    let mut any_match = false;

    if files.is_empty() {
        // Read from stdin
        let reader = io::stdin()?;
        if grep_reader(reader, &re, &opts, "", &mut w).await? {
            any_match = true;
        }
    } else {
        for path in &files {
            let fd = match io::open(os, path, OpenFlags::read()).await {
                Ok(fd) => fd,
                Err(e) => {
                    if !opts.quiet {
                        let mut ew = io::stderr()?;
                        wprintln!(ew, "grep: {}: {}", path, e)?;
                    }
                    continue;
                }
            };
            let reader = io::take_reader(fd)?;
            let prefix = if show_name { path.as_str() } else { "" };
            if grep_reader(reader, &re, &opts, prefix, &mut w).await? {
                any_match = true;
                if opts.quiet {
                    return Ok(0);
                }
            }
        }
    }

    Ok(if any_match { 0 } else { 1 })
}
