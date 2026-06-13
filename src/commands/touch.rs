use crate::prelude::*;

const HELP: &str = "Usage: touch FILE...
Create files or update modification times.";

#[command("touch")]
async fn cmd_touch(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
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
        return Err("touch: missing operand".into());
    }
    for path in &files {
        let st = io::stat(os, path).await;
        if !st.exists {
            // Create empty file
            let fd = io::open(os, path, OpenFlags::write()).await?;
            io::with_process(|p| p.close(fd));
        }
        // For existing files, opening with append and closing updates mtime
        // on most systems without truncating
        else {
            let fd = io::open(os, path, OpenFlags::append()).await?;
            io::with_process(|p| p.close(fd));
        }
    }
    Ok(0)
}
