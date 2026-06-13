use crate::prelude::*;

const HELP: &str = "Usage: ln [-s] TARGET LINK_NAME
Create links.

Options:
  -s  create symbolic link";

#[command("ln")]
async fn cmd_ln(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut symbolic = false;
    let mut paths = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('s') => symbolic = true,
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => paths.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if paths.len() < 2 {
        return Err("ln: missing operand".into());
    }
    let target = &paths[0];
    let link = &paths[1];
    if symbolic {
        io::symlink(os, target, link).await?;
    } else {
        return Err("ln: hard links not supported; use -s".into());
    }
    Ok(0)
}
