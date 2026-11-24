use crate::prelude::*;

const HELP: &str = "Usage: hostname
Print the shell's host name: $HOSTNAME, or localhost if it is unset or empty.";

/// The host name when `$HOSTNAME` is unset or empty.
const DEFAULT_HOSTNAME: &str = "localhost";

#[command("hostname")]
async fn cmd_hostname(_os: &dyn Kernel, args: &[String]) -> CommandResult {
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
    let host = io::with_process(|p| match p.get_env("HOSTNAME") {
        Some(host) if !host.is_empty() => host.to_string(),
        _ => DEFAULT_HOSTNAME.to_string(),
    });
    let mut w = io::stdout()?;
    wprintln!(w, "{}", host)?;
    Ok(0)
}
