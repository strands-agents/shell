use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_cd<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut print = false;
        let dir = if args.is_empty() {
            match proc.env.get("HOME") {
                Some(h) => h.clone(),
                None => {
                    proc.err_msg("strands-shell: cd: HOME not set");
                    return Ok(1);
                }
            }
        } else if args[0] == "-" {
            print = true;
            match proc.env.get("OLDPWD") {
                Some(d) => d.clone(),
                None => {
                    proc.err_msg("strands-shell: cd: OLDPWD not set");
                    return Ok(1);
                }
            }
        } else {
            args[0].clone()
        };

        let oldpwd = proc.cwd.to_string_lossy().to_string();
        if let Err(e) = os.change_dir(proc, &dir).await {
            proc.err_msg(&format!("strands-shell: cd: {dir}: {e}"));
            return Ok(1);
        }
        let newpwd = proc.cwd.to_string_lossy().to_string();
        proc.set_env("OLDPWD", &oldpwd);
        proc.set_env("PWD", &newpwd);

        if print {
            let mut w = io::stdout()?;
            wprintln!(w, "{}", newpwd)?;
        }
        Ok(0)
    })
}
