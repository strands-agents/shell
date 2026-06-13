use crate::prelude::*;

const HELP: &str = "Usage: dirname NAME
Strip last component from NAME.";

#[command("dirname")]
async fn cmd_dirname(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut name = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) if name.is_none() => name = Some(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let name = name.ok_or("dirname: missing operand")?;
    let dir = match name.rfind('/') {
        Some(0) => "/",
        Some(i) => &name[..i],
        None => ".",
    };
    let mut w = io::stdout()?;
    wprintln!(w, "{}", dir)?;
    Ok(0)
}
