use crate::prelude::*;

const HELP: &str = "Usage: rmdir DIRECTORY...
Remove empty directories.";

#[command("rmdir")]
async fn cmd_rmdir(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut dirs = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
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
        return Err("rmdir: missing operand".into());
    }
    for dir in &dirs {
        io::remove_dir(os, dir).await?;
    }
    Ok(0)
}
