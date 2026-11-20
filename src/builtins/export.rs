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
    // `export -n` removes the export instead of adding it.
    let mut export = !readonly;
    let mut names = args;
    while let Some((first, rest)) = names.split_first() {
        match first.as_str() {
            "--" => {
                names = rest;
                break;
            }
            "-n" if !readonly => export = false,
            _ => break,
        }
        names = rest;
    }
    for s in names {
        let key = match s.find('=') {
            Some(eq) => {
                let key = &s[..eq];
                if !proc.set_env(key, &s[eq + 1..]) {
                    return Ok(1);
                }
                key
            }
            None => s.as_str(),
        };
        if readonly {
            proc.mark_readonly(key);
        } else {
            proc.set_exported(key, export);
        }
    }
    Ok(0)
}
