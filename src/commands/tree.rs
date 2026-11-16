use crate::builtins::glob_match;
use crate::os::FdWriter;
use crate::prelude::*;
use std::future::Future;
use std::pin::Pin;

const HELP: &str = "Usage: tree [OPTION]... [DIRECTORY]...
List the contents of directories in a tree-like format.

  -a            include entries starting with .
  -d            list directories only
  -f            print the full path prefix for each entry
  -I PATTERN    do not list entries whose name matches the wildcard PATTERN;
                alternatives are separated by |, and -I may be repeated
  -L LEVEL      descend only LEVEL directories deep
      --noreport  do not print the directory and file counts at the end

Entries are sorted by name. Symbolic links are shown with their target and
are not followed; a link to a directory counts as a directory.";

// Like tree in a UTF-8 locale, the vertical bar is padded with no-break spaces.
const BRANCH: &str = "├── ";
const LAST: &str = "└── ";
const BAR: &str = "│\u{a0}\u{a0} ";
const BLANK: &str = "    ";

struct Opts {
    all: bool,
    dirs_only: bool,
    full_path: bool,
    max_level: Option<usize>,
    ignore: Vec<String>,
}

#[derive(Default)]
struct Counts {
    dirs: usize,
    files: usize,
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

#[command("tree")]
async fn cmd_tree(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut opts = Opts {
        all: false,
        dirs_only: false,
        full_path: false,
        max_level: None,
        ignore: Vec::new(),
    };
    let mut report = true;
    let mut dirs = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('a') => opts.all = true,
            Short('d') => opts.dirs_only = true,
            Short('f') => opts.full_path = true,
            Short('I') => {
                let pattern = parser.value()?.string()?;
                opts.ignore.extend(pattern.split('|').map(str::to_string));
            }
            Short('L') => match parser.value()?.string()?.parse::<usize>() {
                Ok(level) if level > 0 => opts.max_level = Some(level),
                _ => {
                    let mut e = io::stderr()?;
                    wprintln!(e, "tree: Invalid level, must be greater than 0.")?;
                    return Ok(1);
                }
            },
            Long("noreport") => report = false,
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => dirs.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if dirs.is_empty() {
        dirs.push(".".into());
    }
    let mut w = io::stdout()?;
    let mut counts = Counts::default();
    for dir in &dirs {
        // Like tree, an operand that is not a directory is reported on
        // stdout and does not change the exit status.
        if !io::stat(os, dir).await.is_dir {
            wprintln!(w, "{} [error opening dir]", dir)?;
            continue;
        }
        match io::list_dir(os, dir).await {
            Ok(entries) => {
                // With -f, tree drops a trailing slash from the root line.
                let root = match dir.trim_end_matches('/') {
                    trimmed if opts.full_path && !trimmed.is_empty() => trimmed,
                    _ => dir.as_str(),
                };
                wprintln!(w, "{}", root)?;
                list(os, &mut w, dir, entries, "", 1, &opts, &mut counts).await?;
            }
            Err(_) => wprintln!(w, "{} [error opening dir]", dir)?,
        }
    }
    if report {
        wprintln!(w)?;
        if opts.dirs_only {
            wprintln!(w, "{}", plural(counts.dirs, "directory", "directories"))?;
        } else {
            wprintln!(
                w,
                "{}, {}",
                plural(counts.dirs, "directory", "directories"),
                plural(counts.files, "file", "files")
            )?;
        }
    }
    Ok(0)
}

/// Print the `entries` of directory `path`, which is `level` directories
/// below an operand, each line starting with `prefix`.
#[allow(clippy::too_many_arguments)]
fn list<'a>(
    os: &'a dyn Kernel,
    w: &'a mut FdWriter,
    path: &'a str,
    entries: Vec<crate::os::DirEntry>,
    prefix: &'a str,
    level: usize,
    opts: &'a Opts,
    counts: &'a mut Counts,
) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut shown = Vec::new();
        for entry in entries {
            if !opts.all && entry.name.starts_with('.') {
                continue;
            }
            if opts
                .ignore
                .iter()
                .any(|p| glob_match(p.as_bytes(), entry.name.as_bytes()))
            {
                continue;
            }
            let full = join(path, &entry.name);
            // On a direct bind, lstat follows links (#254), so read_link is
            // what tells a link apart.
            let target = io::read_link(os, &full).await.ok();
            let is_dir = io::stat(os, &full).await.is_dir;
            if opts.dirs_only && !is_dir {
                continue;
            }
            shown.push((entry.name, full, is_dir, target));
        }
        shown.sort_by(|a, b| a.0.cmp(&b.0));
        let n = shown.len();
        for (i, (name, full, is_dir, target)) in shown.into_iter().enumerate() {
            let last = i + 1 == n;
            let label = if opts.full_path { &full } else { &name };
            let line = match &target {
                Some(target) => format!("{label} -> {target}"),
                None => label.to_string(),
            };
            let connector = if last { LAST } else { BRANCH };
            if is_dir {
                counts.dirs += 1;
            } else {
                counts.files += 1;
            }
            let descend = is_dir && target.is_none() && opts.max_level.is_none_or(|m| level < m);
            if !descend {
                wprintln!(w, "{prefix}{connector}{line}")?;
                continue;
            }
            match io::list_dir(os, &full).await {
                Ok(children) => {
                    wprintln!(w, "{prefix}{connector}{line}")?;
                    let child_prefix = format!("{prefix}{}", if last { BLANK } else { BAR });
                    list(
                        os,
                        w,
                        &full,
                        children,
                        &child_prefix,
                        level + 1,
                        opts,
                        counts,
                    )
                    .await?;
                }
                Err(_) => wprintln!(w, "{prefix}{connector}{line} [error opening dir]")?,
            }
        }
        Ok(())
    })
}
