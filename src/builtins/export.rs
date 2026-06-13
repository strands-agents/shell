use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};

pub fn builtin_export<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        // Detect if invoked as "readonly"
        // The caller passes args *after* the command name, but we need to know
        // which command was used. We check via a hack: the builtin is registered
        // for both "export" and "readonly", and the dispatch strips the name.
        // We'll use a wrapper approach — see builtin_readonly below.
        do_export(proc, args, false)
    })
}

pub fn builtin_readonly<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move { do_export(proc, args, true) })
}

fn do_export(proc: &mut Process, args: &[String], readonly: bool) -> CommandResult {
    for s in args {
        if let Some(eq) = s.find('=') {
            let key = &s[..eq];
            let val = &s[eq + 1..];
            if !proc.set_env(key, val) {
                return Ok(1);
            }
            if readonly {
                proc.mark_readonly(key);
            }
        } else {
            // `export VAR` / `readonly VAR` without value
            if readonly {
                proc.mark_readonly(s.as_str());
            }
        }
    }
    Ok(0)
}
