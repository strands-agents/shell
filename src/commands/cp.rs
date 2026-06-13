use crate::prelude::*;

const HELP: &str = "Usage: cp [-r] SOURCE... DEST
Copy files and directories.

Options:
  -r, -R  copy directories recursively";

async fn copy_file(os: &dyn Kernel, src: &str, dst: &str) -> std::io::Result<()> {
    let sfd = io::open(os, src, OpenFlags::read()).await?;
    let dfd = io::open(os, dst, OpenFlags::write()).await?;
    let mut reader = io::take_reader(sfd)?;
    let mut writer = io::take_writer(dfd)?;
    tokio::io::copy(&mut reader, &mut writer).await?;
    Ok(())
}

async fn copy_recursive(os: &dyn Kernel, src: &str, dst: &str) -> std::io::Result<()> {
    let st = io::stat(os, src).await;
    if st.is_dir {
        io::create_dir(os, dst).await?;
        for entry in io::list_dir(os, src).await? {
            let s = format!("{}/{}", src, entry.name);
            let d = format!("{}/{}", dst, entry.name);
            Box::pin(copy_recursive(os, &s, &d)).await?;
        }
        Ok(())
    } else {
        copy_file(os, src, dst).await
    }
}

#[command("cp")]
async fn cmd_cp(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut recursive = false;
    let mut paths = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('r') | Short('R') => recursive = true,
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
        return Err("cp: missing operand".into());
    }
    let dest = paths.last().unwrap().clone();
    let sources = &paths[..paths.len() - 1];
    let dest_is_dir = io::stat(os, &dest).await.is_dir;

    if sources.len() > 1 && !dest_is_dir {
        return Err("cp: target is not a directory".into());
    }

    let mut exit = 0;
    for src in sources {
        let st = io::stat(os, src).await;
        if st.is_dir && !recursive {
            let mut ew = io::stderr()?;
            wprintln!(ew, "cp: -r not specified; omitting directory '{}'", src)?;
            exit = 1;
            continue;
        }
        let target = if dest_is_dir {
            let name = src.rsplit('/').next().unwrap_or(src);
            format!("{}/{}", dest, name)
        } else {
            dest.clone()
        };
        if recursive && st.is_dir {
            copy_recursive(os, src, &target).await?;
        } else {
            copy_file(os, src, &target).await?;
        }
    }
    Ok(exit)
}
