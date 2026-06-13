use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_set<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.is_empty() {
            // Print all variables
            let mut w = io::stdout()?;
            let mut vars: Vec<_> = proc.env.iter().collect();
            vars.sort_by_key(|(k, _)| (*k).clone());
            for (k, v) in vars {
                wprintln!(w, "{}={}", k, v)?;
            }
            return Ok(0);
        }

        // `set --` clears positional params; `set -- a b c` sets them
        if args[0] == "--" {
            proc.args = args[1..].to_vec();
            return Ok(0);
        }

        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with('-') || a.starts_with('+') {
                let enable = a.starts_with('-');
                for ch in a[1..].chars() {
                    match ch {
                        'e' => proc.opt_errexit = enable,
                        'u' => proc.opt_nounset = enable,
                        'x' => proc.opt_xtrace = enable,
                        _ => {
                            proc.err_msg(&format!("strands-shell: set: -{ch}: unsupported option"));
                            return Ok(2);
                        }
                    }
                }
            } else {
                // Positional params
                proc.args = args[i..].to_vec();
                return Ok(0);
            }
            i += 1;
        }

        Ok(0)
    })
}
