use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

/// Search PATH for an executable named `name` using the Kernel abstraction.
async fn find_in_path(os: &dyn Kernel, proc: &Process, name: &str) -> Option<String> {
    let path_var = proc.env.get("PATH")?;
    for dir in path_var.split(':') {
        let full = if dir.is_empty() {
            format!("./{name}")
        } else {
            format!("{dir}/{name}")
        };
        if os.is_executable(proc, &full).await {
            return Some(full);
        }
    }
    None
}

pub fn builtin_hash<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.is_empty() {
            // List all hashed commands
            let mut w = io::stdout()?;
            let table = proc.hash_table.clone();
            if table.is_empty() {
                return Ok(0);
            }
            let mut entries: Vec<_> = table.iter().collect();
            entries.sort_by_key(|(k, _)| (*k).clone());
            for (name, path) in entries {
                wprintln!(w, "{name}={path}")?;
            }
            return Ok(0);
        }

        // Check for -r flag
        let mut names = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-r" => {
                    Arc::make_mut(&mut proc.hash_table).clear();
                }
                _ => names.push(&args[i]),
            }
            i += 1;
        }

        let mut status = 0;
        for name in names {
            match find_in_path(os, proc, name).await {
                Some(path) => {
                    Arc::make_mut(&mut proc.hash_table).insert(name.clone(), path);
                }
                None => {
                    proc.err_msg(&format!("strands-shell: hash: {name}: not found"));
                    status = 1;
                }
            }
        }
        Ok(status)
    })
}
