use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::exec::{CommandLookup, find_all_in_path, lookup_command};
use crate::os::{Kernel, Process};
use crate::prelude::*;

const USAGE: &str = "usage: which [-a] name ...";

/// `which [-a] name ...`: print where each command is found, resolved as
/// `command -v` resolves it except that shell functions are not consulted.
/// A builtin or registered command prints its bare name, as `command -v` does.
/// `-a` also prints every further match in PATH. Missing names print nothing;
/// a name containing `/` is checked as a path. The status is 0 when every
/// name is found and 1 otherwise.
pub fn builtin_which<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut all = false;
        let mut names = args;
        while let Some(arg) = names.first() {
            if arg == "--" {
                names = &names[1..];
                break;
            }
            if arg.len() < 2 || !arg.starts_with('-') {
                break;
            }
            let bad = if arg.starts_with("--") {
                Some(arg.clone())
            } else {
                arg[1..]
                    .chars()
                    .find(|&c| c != 'a')
                    .map(|c| format!("-{c}"))
            };
            if let Some(bad) = bad {
                proc.err_msg(&format!("strands-shell: which: {bad}: invalid option"));
                proc.err_msg(USAGE);
                return Ok(2);
            }
            all = true;
            names = &names[1..];
        }
        if names.is_empty() {
            proc.err_msg(USAGE);
            return Ok(1);
        }

        let mut w = io::stdout()?;
        let mut status = 0;
        for name in names {
            let mut printed = Vec::new();
            match lookup_command(os, proc, name, false).await {
                Some(CommandLookup::Builtin) => printed.push(name.clone()),
                Some(CommandLookup::Path(path)) => printed.push(path),
                Some(CommandLookup::Function) | None => {}
            }
            if all && !name.contains('/') {
                for path in find_all_in_path(os, proc, name, true).await {
                    if !printed.contains(&path) {
                        printed.push(path);
                    }
                }
            }
            if printed.is_empty() {
                status = 1;
            }
            for line in printed {
                wprintln!(w, "{line}")?;
            }
        }
        Ok(status)
    })
}
