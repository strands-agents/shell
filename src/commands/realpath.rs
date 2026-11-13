use super::readlink::{Mode, canonicalize};
use crate::os::FdWriter;
use crate::prelude::*;

const HELP: &str = "Usage: realpath [OPTION]... FILE...
Print the resolved absolute file name;
all but the last component must exist

  -e, --canonicalize-existing  all components of the path must exist
  -m, --canonicalize-missing   no path components need exist or be a directory
  -q, --quiet                  suppress most error messages
      --relative-to=DIR        print the resolved path relative to DIR
      --relative-base=DIR      print absolute paths unless paths below DIR
  -s, --strip, --no-symlinks   don't expand symlinks
  -z, --zero                   end each output line with NUL, not newline";

#[command("realpath")]
async fn cmd_realpath(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut paths = Vec::new();
    let mut mode = Mode::AllButLast;
    let mut follow_links = true;
    let mut quiet = false;
    let mut end = '\n';
    let mut relative_to = None;
    let mut relative_base = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Short('e') | Long("canonicalize-existing") => mode = Mode::Existing,
            Short('m') | Long("canonicalize-missing") => mode = Mode::Missing,
            Short('q') | Long("quiet") => quiet = true,
            Short('s') | Long("strip") | Long("no-symlinks") => follow_links = false,
            Short('z') | Long("zero") => end = '\0',
            Long("relative-to") => relative_to = Some(parser.value()?.string()?),
            Long("relative-base") => relative_base = Some(parser.value()?.string()?),
            Value(val) => paths.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if paths.is_empty() {
        return Err("missing operand".into());
    }

    // Like GNU realpath: --relative-base alone also sets --relative-to, and
    // a --relative-to outside --relative-base makes every path absolute.
    let resolve_dir = async |dir: &str| {
        canonicalize(os, dir, mode, follow_links)
            .await
            .map_err(|e| format!("{}: {}", quote(dir), e))
    };
    let (to, base) = match (relative_to.as_deref(), relative_base.as_deref()) {
        (None, None) => (None, None),
        (Some(to), None) => (Some(resolve_dir(to).await?), None),
        (None, Some(base)) => {
            let base = resolve_dir(base).await?;
            (Some(base.clone()), Some(base))
        }
        (Some(to), Some(base)) => {
            let to = resolve_dir(to).await?;
            let base = resolve_dir(base).await?;
            if is_below(&base, &to) {
                (Some(to), Some(base))
            } else {
                (None, None)
            }
        }
    };

    let mut w = io::stdout()?;
    let mut errors: Option<FdWriter> = None;
    let mut status = 0;
    for path in &paths {
        let resolved = match canonicalize(os, path, mode, follow_links).await {
            Ok(resolved) => resolved,
            Err(e) => {
                status = 1;
                if !quiet {
                    if errors.is_none() {
                        errors = Some(io::stderr()?);
                    }
                    let ew = errors.as_mut().expect("stderr was just taken");
                    wprintln!(ew, "realpath: {}: {}", quote(path), e)?;
                }
                continue;
            }
        };
        let out = match &to {
            Some(to) if base.as_deref().is_none_or(|b| is_below(b, &resolved)) => {
                relative(&resolved, to)
            }
            _ => resolved,
        };
        wprint!(w, "{}{}", out, end)?;
    }
    Ok(status)
}

/// Whether the absolute path `path` is `dir` or lies under it.
fn is_below(dir: &str, path: &str) -> bool {
    dir == "/"
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// `path` relative to the directory `dir`, both absolute and canonical.
fn relative(path: &str, dir: &str) -> String {
    let path: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let dir: Vec<&str> = dir.split('/').filter(|c| !c.is_empty()).collect();
    let common = path.iter().zip(&dir).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; dir.len() - common];
    parts.extend(&path[common..]);
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Quote a file name for an error message the way GNU does for an empty one.
fn quote(path: &str) -> &str {
    if path.is_empty() { "''" } else { path }
}
