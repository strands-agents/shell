use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};

pub fn builtin_local<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        for arg in args {
            if let Some(eq) = arg.find('=') {
                proc.set_local(&arg[..eq], &arg[eq + 1..]);
            } else {
                proc.declare_local(arg);
            }
        }
        Ok(0)
    })
}
