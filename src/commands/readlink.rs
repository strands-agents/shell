use crate::prelude::*;

const HELP: &str = "Usage: readlink FILE
Print the target of a symbolic link.";

#[command("readlink")]
async fn cmd_readlink(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut path = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) if path.is_none() => path = Some(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let path = path.ok_or("readlink: missing operand")?;
    let target = io::read_link(os, &path).await?;
    let mut w = io::stdout()?;
    wprintln!(w, "{}", target)?;
    Ok(0)
}
