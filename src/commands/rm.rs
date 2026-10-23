use crate::os::FdWriter;
use crate::prelude::*;

const HELP: &str = "Usage: rm [-rf] FILE...
Remove files or directories.

Options:
  -f  ignore nonexistent files
  -r  remove directories and their contents recursively";

/// Stderr, taken on the first failure and reused for the rest, because a
/// process can take a descriptor only once.
#[derive(Default)]
struct Errors(Option<FdWriter>);

impl Errors {
    async fn report(&mut self, path: &str, e: impl std::fmt::Display) -> std::io::Result<()> {
        if self.0.is_none() {
            self.0 = Some(io::stderr()?);
        }
        let ew = self.0.as_mut().expect("stderr was just taken");
        wprintln!(ew, "rm: {}: {}", path, e)
    }
}

/// Remove `path` and everything under it, reporting each failure by its own
/// path. Returns `Ok(false)` if anything could not be removed.
async fn remove_recursive(
    os: &dyn Kernel,
    errors: &mut Errors,
    path: &str,
) -> std::io::Result<bool> {
    let st = io::lstat(os, path).await;
    if !st.is_dir || st.is_symlink {
        return match io::remove_file(os, path).await {
            Ok(()) => Ok(true),
            Err(e) => errors.report(path, e).await.map(|()| false),
        };
    }
    let entries = match io::list_dir(os, path).await {
        Ok(entries) => entries,
        Err(e) => return errors.report(path, e).await.map(|()| false),
    };
    let mut ok = true;
    for entry in entries {
        let child = format!("{}/{}", path, entry.name);
        ok &= Box::pin(remove_recursive(os, errors, &child)).await?;
    }
    // Still try the directory after a child failed, but report only the
    // child, as coreutils does.
    if let Err(e) = io::remove_dir(os, path).await {
        if ok {
            errors.report(path, e).await?;
        }
        ok = false;
    }
    Ok(ok)
}

#[command("rm")]
async fn cmd_rm(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut force = false;
    let mut recursive = false;
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('f') => force = true,
            Short('r') | Short('R') => recursive = true,
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if files.is_empty() {
        return Err("rm: missing operand".into());
    }
    let mut errors = Errors::default();
    let mut code = 0;
    for path in &files {
        let st = io::lstat(os, path).await;
        if !st.exists {
            if !force {
                errors.report(path, "No such file or directory").await?;
                code = 1;
            }
            continue;
        }
        if st.is_dir && !st.is_symlink {
            if !recursive {
                errors.report(path, "is a directory").await?;
                code = 1;
                continue;
            }
            if !remove_recursive(os, &mut errors, path).await? {
                code = 1;
            }
        } else if let Err(e) = io::remove_file(os, path).await {
            errors.report(path, e).await?;
            code = 1;
        }
    }
    Ok(code)
}
