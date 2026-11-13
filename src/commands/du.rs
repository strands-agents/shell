use super::ls::human_size;
use crate::os::FdWriter;
use crate::prelude::*;
use std::future::Future;
use std::pin::Pin;

const HELP: &str = "Usage: du [OPTION]... [FILE]...
Summarize the space used by each FILE, recursively for directories.

  -a, --all             write counts for all files, not just directories
  -b, --bytes           print sizes in bytes
  -c, --total           produce a grand total
  -d, --max-depth=N     print the total for a directory (or file, with --all)
                        only if it is N or fewer levels below the command
                        line argument; --max-depth=0 is the same as
                        --summarize
  -h, --human-readable  print sizes in human readable format (e.g., 1K 234M 2G)
  -k                    print sizes in 1024-byte blocks (the default)
  -s, --summarize       display only a total for each argument
      --apparent-size   accepted for compatibility; sizes are always apparent

The virtual filesystem does not allocate blocks, so the space a file uses is
its apparent size: the number of bytes in it. Like GNU du --apparent-size,
totals add up bytes and are rounded up to whole 1K blocks only when printed.
Symbolic links are not followed.";

#[derive(Clone, Copy)]
enum Unit {
    Kilo,
    Bytes,
    Human,
}

struct Opts {
    all: bool,
    max_depth: Option<usize>,
    unit: Unit,
}

/// Where a walk reports sizes and errors.
struct Out<'a> {
    w: &'a mut FdWriter,
    e: &'a mut FdWriter,
    code: &'a mut i32,
}

fn format_size(bytes: u64, unit: Unit) -> String {
    match unit {
        Unit::Kilo => bytes.div_ceil(1024).to_string(),
        Unit::Bytes => bytes.to_string(),
        Unit::Human => human_size(bytes),
    }
}

fn usage_error(msg: &str) -> String {
    format!("du: {msg}\nTry 'du --help' for more information.")
}

#[command("du")]
async fn cmd_du(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut all = false;
    let mut summarize = false;
    let mut total = false;
    let mut max_depth = None;
    let mut unit = Unit::Kilo;
    let mut paths = Vec::new();
    // Stderr is taken only after parsing, so the shell can report a bad option.
    while let Some(arg) = parser.next()? {
        match arg {
            Short('a') | Long("all") => all = true,
            Short('b') | Long("bytes") => unit = Unit::Bytes,
            Short('c') | Long("total") => total = true,
            Short('d') | Long("max-depth") => {
                let val = parser.value()?.string()?;
                match val.parse::<usize>() {
                    Ok(n) => max_depth = Some(n),
                    Err(_) => {
                        let msg = format!("invalid maximum depth '{val}'");
                        wprintln!(io::stderr()?, "{}", usage_error(&msg))?;
                        return Ok(1);
                    }
                }
            }
            Short('h') | Long("human-readable") => unit = Unit::Human,
            Short('k') => unit = Unit::Kilo,
            Short('s') | Long("summarize") => summarize = true,
            Long("apparent-size") => {}
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => paths.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mut e = io::stderr()?;
    if summarize {
        if all {
            let msg = "cannot both summarize and show all entries";
            wprintln!(e, "{}", usage_error(msg))?;
            return Ok(1);
        }
        match max_depth {
            Some(0) => wprintln!(
                e,
                "du: warning: summarizing is the same as using --max-depth=0"
            )?,
            Some(n) => {
                let msg = format!("warning: summarizing conflicts with --max-depth={n}");
                wprintln!(e, "{}", usage_error(&msg))?;
                return Ok(1);
            }
            None => {}
        }
        max_depth = Some(0);
    }
    if paths.is_empty() {
        paths.push(".".into());
    }
    let opts = Opts {
        all,
        max_depth,
        unit,
    };
    let mut w = io::stdout()?;
    let mut code = 0;
    let mut grand_total = 0;
    for path in &paths {
        if !io::lstat(os, path).await.exists {
            wprintln!(e, "du: cannot access '{}': No such file or directory", path)?;
            code = 1;
            continue;
        }
        let mut out = Out {
            w: &mut w,
            e: &mut e,
            code: &mut code,
        };
        grand_total += walk(os, path, 0, &opts, &mut out).await?;
    }
    if total {
        wprintln!(w, "{}\ttotal", format_size(grand_total, unit))?;
    }
    Ok(code)
}

/// Add up the bytes under `path`, which is `depth` levels below an operand,
/// and print a line for it and for each entry below it that the options
/// show. Entries are visited in name order and a directory follows its
/// contents, like GNU du.
fn walk<'a>(
    os: &'a dyn Kernel,
    path: &'a str,
    depth: usize,
    opts: &'a Opts,
    out: &'a mut Out<'_>,
) -> Pin<Box<dyn Future<Output = std::io::Result<u64>> + Send + 'a>> {
    Box::pin(async move {
        // On a direct bind, lstat follows links (#254), so read_link is what
        // tells a link apart. A link's size is the length of its target.
        let (mut size, is_dir) = match io::read_link(os, path).await {
            Ok(target) => (target.len() as u64, false),
            Err(_) => {
                let st = io::lstat(os, path).await;
                (st.len, st.is_dir)
            }
        };
        if is_dir {
            match io::list_dir(os, path).await {
                Ok(mut entries) => {
                    entries.sort_by(|a, b| a.name.cmp(&b.name));
                    let base = path.trim_end_matches('/');
                    for entry in entries {
                        let child = format!("{base}/{}", entry.name);
                        size += walk(os, &child, depth + 1, opts, out).await?;
                    }
                }
                Err(err) => {
                    wprintln!(out.e, "du: cannot read directory '{}': {}", path, err)?;
                    *out.code = 1;
                }
            }
        }
        let shown = opts.max_depth.is_none_or(|max| depth <= max);
        if shown && (is_dir || opts.all || depth == 0) {
            wprintln!(out.w, "{}\t{}", format_size(size, opts.unit), path)?;
        }
        Ok(size)
    })
}
