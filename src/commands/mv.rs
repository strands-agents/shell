use crate::prelude::*;

const HELP: &str = "Usage: mv SOURCE... DEST
Move (rename) files and directories.";

#[command("mv")]
async fn cmd_mv(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut paths = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
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
        return Err("mv: missing operand".into());
    }
    let dest = paths.last().unwrap().clone();
    let sources = &paths[..paths.len() - 1];
    let dest_is_dir = io::stat(os, &dest).await.is_dir;

    if sources.len() > 1 && !dest_is_dir {
        return Err("mv: target is not a directory".into());
    }

    for src in sources {
        let target = if dest_is_dir {
            let name = src.rsplit('/').next().unwrap_or(src);
            format!("{}/{}", dest, name)
        } else {
            dest.clone()
        };
        io::rename(os, src, &target).await?;
    }
    Ok(0)
}
