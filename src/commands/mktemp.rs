use crate::prelude::*;

const HELP: &str = "Usage: mktemp [-d] [-p DIR] [TEMPLATE]
Create a temporary file or directory.

Options:
  -d        create a directory instead of a file
  -p DIR    use DIR as the parent (default: $TMPDIR or /tmp)

TEMPLATE should contain 'XXXXXX' which is replaced with random chars.
Default template: tmp.XXXXXX";

async fn random_suffix(os: &dyn Kernel, len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut buf = vec![0u8; len];
    let mut proc = io::with_process(|p| p.fork());
    if let Ok(fd) = os.open(&mut proc, "/dev/urandom", OpenFlags::read()).await
        && let Ok(mut reader) = proc.take_reader(fd)
    {
        use tokio::io::AsyncReadExt;
        let _ = reader.read_exact(&mut buf).await;
    }
    buf.iter()
        .map(|b| CHARS[(*b as usize) % CHARS.len()] as char)
        .collect()
}

#[command("mktemp")]
async fn cmd_mktemp(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut dir_mode = false;
    let mut parent = None;
    let mut template = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') => dir_mode = true,
            Short('p') => parent = Some(parser.value()?.string()?),
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) if template.is_none() => template = Some(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let tmpl = template.as_deref().unwrap_or("tmp.XXXXXX");
    let (base_dir, tmpl_name) = if let Some(pos) = tmpl.rfind('/') {
        (tmpl[..pos].to_string(), &tmpl[pos + 1..])
    } else {
        (
            parent
                .or_else(|| io::with_process(|p| p.get_env("TMPDIR").map(String::from)))
                .unwrap_or_else(|| "/tmp".into()),
            tmpl,
        )
    };

    // Count trailing 'X's to replace with random chars. If the template has
    // none, append a 6-X suffix (GNU behavior); never clamp the count past the
    // template length, which would underflow the slice below (e.g. `mktemp X`).
    let trailing_x = tmpl_name.chars().rev().take_while(|&c| c == 'X').count();
    let (prefix, x_count) = if trailing_x == 0 {
        (tmpl_name, 6)
    } else {
        (&tmpl_name[..tmpl_name.len() - trailing_x], trailing_x)
    };
    let name = format!("{}{}", prefix, random_suffix(os, x_count).await);
    let path = format!("{}/{}", base_dir, name);

    if dir_mode {
        io::create_dir(os, &path).await?;
    } else {
        let fd = io::open(os, &path, OpenFlags::write()).await?;
        io::with_process(|p| p.close(fd));
    }
    let mut w = io::stdout()?;
    wprintln!(w, "{}", path)?;
    Ok(0)
}
