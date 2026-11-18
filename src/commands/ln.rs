use crate::prelude::*;
use std::path::Path;

const HELP: &str = "Usage: ln [-sf] TARGET LINK_NAME
Create links.

If LINK_NAME is a directory, the link is created inside it.

Options:
  -s           create symbolic link
  -f, --force  remove existing destination files";

#[command("ln")]
async fn cmd_ln(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut symbolic = false;
    let mut force = false;
    let mut paths = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('s') => symbolic = true,
            Short('f') | Long("force") => force = true,
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
    let mut link = paths[1].clone();
    let link_is_dir = io::stat(os, &link).await.is_dir;
    if link.ends_with('/') && !link_is_dir {
        if io::lstat(os, link.trim_end_matches('/')).await.exists {
            return Err(format!("failed to access '{link}': Not a directory").into());
        }
        return Err(
            format!("failed to create symbolic link '{link}': No such file or directory").into(),
        );
    }
    if link_is_dir {
        let name = Path::new(target)
            .file_name()
            .map_or(target.as_str(), |n| n.to_str().unwrap_or(target));
        link = format!("{}/{}", link.trim_end_matches('/'), name);
    }
    let dest = io::lstat(os, &link).await;
    if force && dest.exists {
        let src = io::stat(os, target).await;
        if src.exists && src.ino != 0 && src.dev == dest.dev && src.ino == dest.ino {
            return Err(format!("'{target}' and '{link}' are the same file").into());
        }
    }
    if !symbolic {
        return Err("ln: hard links not supported; use -s".into());
    }
    // Remove the destination only once creating the link has failed because it exists, so a
    // kernel that cannot create the link at all leaves the destination untouched.
    match io::symlink(os, target, &link).await {
        Err(e) if force && e.kind() == std::io::ErrorKind::AlreadyExists => {
            if dest.is_dir {
                return Err(format!("{link}: cannot overwrite directory").into());
            }
            io::remove_file(os, &link).await?;
            io::symlink(os, target, &link).await?;
        }
        result => result?,
    }
    Ok(0)
}
