use crate::prelude::*;

const HELP: &str = "Usage: echo [STRING]...
Display a line of text.";

#[command("echo")]
async fn cmd_echo(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut parts = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => parts.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let mut w = io::stdout()?;
    wprintln!(w, "{}", parts.join(" "))?;
    Ok(0)
}
