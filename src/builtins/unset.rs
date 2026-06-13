use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};

pub fn builtin_unset<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut func_mode = false;
        let mut names = Vec::new();
        for a in args {
            match a.as_str() {
                "-f" => func_mode = true,
                "-v" => func_mode = false,
                _ => names.push(a.as_str()),
            }
        }
        for name in names {
            if func_mode {
                proc.unset_function(name);
            } else {
                proc.unset_env(name);
            }
        }
        Ok(0)
    })
}
