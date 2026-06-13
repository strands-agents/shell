use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_pwd<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut physical = false;
        for arg in args {
            match arg.as_str() {
                "-L" => physical = false,
                "-P" => physical = true,
                _ => {
                    proc.err_msg(&format!("strands-shell: pwd: bad option: {arg}"));
                    return Ok(2);
                }
            }
        }
        let mut w = io::stdout()?;
        if physical {
            match os.canonicalize(proc, ".").await {
                Ok(p) => wprintln!(w, "{}", p.display())?,
                Err(_) => wprintln!(w, "{}", proc.cwd.display())?,
            }
        } else {
            wprintln!(w, "{}", proc.cwd.display())?;
        }
        Ok(0)
    })
}
