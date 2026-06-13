use crate::prelude::*;

const HELP: &str = "Usage: sleep SECONDS
Pause for SECONDS (accepts decimals).";

#[command("sleep")]
async fn cmd_sleep(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut secs = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) if secs.is_none() => secs = Some(val.string()?.parse::<f64>()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let secs = secs.ok_or("sleep: missing operand")?;
    let sleep_dur = std::time::Duration::from_secs_f64(secs);

    #[cfg(target_arch = "wasm32")]
    {
        // WASI supports std::thread::sleep via poll_oneoff
        std::thread::sleep(sleep_dur);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let deadline = io::with_process(|p| p.deadline);
        if let Some(dl) = deadline {
            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => {}
                _ = tokio::time::sleep_until(dl) => {}
            }
        } else {
            tokio::time::sleep(sleep_dur).await;
        }
    }

    Ok(0)
}
