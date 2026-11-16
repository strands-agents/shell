use crate::prelude::*;
#[cfg(target_arch = "wasm32")]
use std::time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::Instant;

const HELP: &str = "Usage: yes [STRING]...
Repeatedly output a line with all specified STRING(s), or 'y'.";

/// Lines are written in chunks of at least this many bytes.
const CHUNK: usize = 8192;

#[command("yes")]
async fn cmd_yes(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut words = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => words.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let line = if words.is_empty() {
        "y\n".to_string()
    } else {
        format!("{}\n", words.join(" "))
    };
    let chunk = line.repeat(CHUNK.div_ceil(line.len()));
    let deadline = io::with_process(|p| p.deadline);
    let mut w = io::stdout()?;
    loop {
        // yes never ends by itself, so it stops at the shell's deadline.
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err("execution timeout exceeded".into());
        }
        match w.write_all(chunk.as_bytes()).await {
            Ok(()) => {}
            // A reader that went away stops yes quietly, as SIGPIPE does.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(1),
            Err(e) => return Err(e.into()),
        }
    }
}
