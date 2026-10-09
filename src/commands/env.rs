use crate::prelude::*;

const HELP: &str = "Usage: env
Print the environment.";

#[command("env")]
async fn cmd_env(os: &dyn Kernel, args: &[String]) -> CommandResult {
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
    // `env` enumerates the whole environment table — gate it as an explicit
    // environment read. Shell `$VAR` interpolation is deliberately not gated.
    os.check_policy("env:read", &[("name", "*")])?;
    let mut vars: Vec<(String, String)> = io::with_process(|proc| {
        proc.env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    });
    vars.sort();
    let mut w = io::stdout()?;
    for (k, v) in &vars {
        wprintln!(w, "{}={}", k, v)?;
    }
    Ok(0)
}
