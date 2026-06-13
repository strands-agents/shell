use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_type<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut status = 0;
        let mut w = io::stdout()?;
        for name in args {
            if let Some(val) = proc.aliases.get(name.as_str()) {
                wprintln!(w, "{} is an alias for {}", name, val)?;
            } else if is_special_builtin(name) {
                wprintln!(w, "{} is a special shell builtin", name)?;
            } else if crate::builtins::lookup(name).is_some() {
                wprintln!(w, "{} is a shell builtin", name)?;
            } else if proc.get_function(name).is_some() {
                wprintln!(w, "{} is a shell function", name)?;
            } else if let Some(path) = proc.hash_table.get(name.as_str()) {
                wprintln!(w, "{} is hashed ({})", name, path)?;
            } else if crate::commands::lookup(name).is_some() {
                wprintln!(w, "{} is a shell builtin", name)?;
            } else {
                wprintln!(w, "strands-shell: type: {}: not found", name)?;
                status = 1;
            }
        }
        Ok(status)
    })
}

fn is_special_builtin(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "continue"
            | "exit"
            | "return"
            | "eval"
            | "exec"
            | "."
            | ":"
            | "set"
            | "shift"
            | "export"
            | "readonly"
            | "trap"
            | "unset"
    )
}
