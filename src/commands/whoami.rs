use crate::prelude::*;

const HELP: &str = "Usage: whoami
Print the user name of the shell's user.";

/// The user name when `$USER` is unset or empty.
const DEFAULT_USER: &str = "lash";

#[command("whoami")]
async fn cmd_whoami(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    if let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            _ => return Err(arg.unexpected().into()),
        }
    }
    let user = io::with_process(|p| match p.get_env("USER") {
        Some(user) if !user.is_empty() => user.to_string(),
        _ => DEFAULT_USER.to_string(),
    });
    let mut w = io::stdout()?;
    wprintln!(w, "{}", user)?;
    Ok(0)
}
