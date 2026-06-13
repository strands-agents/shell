use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};

pub fn builtin_wait<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    _args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut last = 0;
        let jobs = std::mem::take(&mut proc.bg_jobs);
        for handle in jobs {
            let (code, stdout, stderr) = handle.await.unwrap_or((1, String::new(), String::new()));
            last = code;
            if proc.capture {
                proc.captured_output.push_str(&stdout);
                proc.captured_stderr.push_str(&stderr);
            }
        }
        proc.last_exit = last;
        Ok(last)
    })
}
