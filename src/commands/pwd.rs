use crate::prelude::*;

const HELP: &str = "Usage: pwd
Print the current working directory.";

#[command("pwd")]
async fn cmd_pwd(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    if let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mut w = io::stdout()?;
    let cwd = io::with_process(|p| p.cwd.display().to_string());
    wprintln!(w, "{}", cwd)?;
    Ok(0)
}
