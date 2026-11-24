use crate::prelude::*;

const HELP: &str = "Usage: mkdir [-p] DIRECTORY...
Create directories.

Options:
  -p  create parent directories as needed";

#[command("mkdir")]
async fn cmd_mkdir(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut parents = false;
    let mut dirs = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('p') => parents = true,
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
        return Err("mkdir: missing operand".into());
    }
    // Like GNU, report each directory that can't be created and go on with
    // the rest. Stderr is taken once, on the first failure.
    let mut ew = None;
    for dir in &dirs {
        if let Err(e) = make_dir(os, dir, parents).await {
            if ew.is_none() {
                ew = Some(io::stderr()?);
            }
            let ew = ew.as_mut().expect("stderr was just taken");
            wprintln!(ew, "strands-shell: mkdir: {}", e)?;
        }
    }
    Ok(i32::from(ew.is_some()))
}

/// Create `dir`, and with `parents` the missing directories above it.
async fn make_dir(os: &dyn Kernel, dir: &str, parents: bool) -> std::io::Result<()> {
    // An empty operand goes to the kernel, which refuses it; the walk
    // below would read it as `/`.
    if !parents || dir.is_empty() {
        return io::create_dir(os, dir).await;
    }
    // Build each component
    let parts: Vec<&str> = dir.split('/').collect();
    let target = parts.iter().rposition(|part| !part.is_empty());
    let mut path = String::new();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() && path.is_empty() {
            path.push('/');
            continue;
        }
        if !path.is_empty() && !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(part);
        let is_ancestor = target.is_some_and(|target| index < target);
        let skip = if is_ancestor {
            match io::try_stat(os, &path).await {
                Ok(stat) => stat.exists,
                Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
            }
        } else {
            io::stat(os, &path).await.exists
        };
        if skip {
            continue;
        }
        io::create_dir(os, &path).await?;
    }
    Ok(())
}
