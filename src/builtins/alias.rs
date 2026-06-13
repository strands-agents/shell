use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_alias<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.is_empty() {
            let mut w = io::stdout()?;
            let mut entries: Vec<_> = proc.aliases.iter().collect();
            entries.sort_by_key(|(k, _)| (*k).clone());
            for (name, val) in entries {
                wprintln!(w, "{}='{}'", name, val.replace('\'', "'\\''"))?;
            }
            return Ok(0);
        }
        let mut ret = 0;
        for arg in args {
            if let Some(eq) = arg.find('=') {
                let (name, val) = arg.split_at(eq);
                proc.set_alias(name, &val[1..]);
            } else if let Some(val) = proc.aliases.get(arg.as_str()) {
                let mut w = io::stdout()?;
                wprintln!(w, "{}='{}'", arg, val.replace('\'', "'\\''"))?;
            } else {
                proc.err_msg(&format!("strands-shell: alias: {}: not found", arg));
                ret = 1;
            }
        }
        Ok(ret)
    })
}

pub fn builtin_unalias<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.first().map(|s| s.as_str()) == Some("-a") {
            proc.clear_aliases();
            return Ok(0);
        }
        let mut ret = 0;
        for arg in args {
            if !proc.unset_alias(arg) {
                proc.err_msg(&format!("strands-shell: unalias: {}: not found", arg));
                ret = 1;
            }
        }
        Ok(ret)
    })
}
