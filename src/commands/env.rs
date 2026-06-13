use crate::prelude::*;

const HELP: &str = "Usage: env
Print the environment.";

#[command("env")]
async fn cmd_env(_os: &dyn Kernel, args: &[String]) -> CommandResult {
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
