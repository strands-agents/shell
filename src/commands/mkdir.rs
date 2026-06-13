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
    for dir in &dirs {
        if parents {
            // Build each component
            let mut path = String::new();
            for part in dir.split('/') {
                if part.is_empty() && path.is_empty() {
                    path.push('/');
                    continue;
                }
                if !path.is_empty() && !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(part);
                let st = io::stat(os, &path).await;
                if !st.exists {
                    io::create_dir(os, &path).await?;
                }
            }
        } else {
            io::create_dir(os, dir).await?;
        }
    }
    Ok(0)
}
