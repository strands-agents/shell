use crate::prelude::*;

const HELP: &str = "Usage: rm [-rf] FILE...
Remove files or directories.

Options:
  -f  ignore nonexistent files
  -r  remove directories and their contents recursively";

async fn remove_recursive(os: &dyn Kernel, path: &str) -> std::io::Result<()> {
    let st = io::lstat(os, path).await;
    if st.is_dir && !st.is_symlink {
        for entry in io::list_dir(os, path).await? {
            let child = format!("{}/{}", path, entry.name);
            Box::pin(remove_recursive(os, &child)).await?;
        }
        io::remove_dir(os, path).await
    } else {
        io::remove_file(os, path).await
    }
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
    let mut code = 0;
    for path in &files {
        let st = io::lstat(os, path).await;
        if !st.exists {
            if !force {
                let mut ew = io::stderr()?;
                wprintln!(ew, "rm: {}: No such file or directory", path)?;
                code = 1;
            }
            continue;
        }
        if st.is_dir && !st.is_symlink {
            if !recursive {
                let mut ew = io::stderr()?;
                wprintln!(ew, "rm: {}: is a directory", path)?;
                code = 1;
                continue;
            }
            if let Err(e) = remove_recursive(os, path).await
                && !force
            {
                let mut ew = io::stderr()?;
                wprintln!(ew, "rm: {}: {}", path, e)?;
                code = 1;
            }
        } else if let Err(e) = io::remove_file(os, path).await
            && !force
        {
            let mut ew = io::stderr()?;
            wprintln!(ew, "rm: {}: {}", path, e)?;
            code = 1;
        }
    }
    Ok(code)
}
