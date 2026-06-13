use crate::prelude::*;

const HELP: &str = "Usage: basename NAME [SUFFIX]
Strip directory and optional SUFFIX from NAME.";

#[command("basename")]
async fn cmd_basename(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut values = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => values.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if values.is_empty() {
        return Err("basename: missing operand".into());
    }
    let name = values[0].trim_end_matches('/');
    let mut base = name.rsplit('/').next().unwrap_or(name);
    if let Some(suffix) = values.get(1)
        && !suffix.is_empty()
        && base.len() > suffix.len()
        && let Some(stripped) = base.strip_suffix(suffix.as_str())
    {
        base = stripped;
    }
    let mut w = io::stdout()?;
    wprintln!(w, "{}", base)?;
    Ok(0)
}
