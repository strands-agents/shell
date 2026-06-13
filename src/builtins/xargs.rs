use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

fn exec_fork(proc: &mut Process) -> Process {
    let mut sub = proc.fork();
    sub.depth += 1;
    sub.capture = true;
    sub
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn builtin_xargs<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut null_delim = false;
        let mut replace: Option<String> = None;
        let mut max_args: usize = 0;
        let mut delim: Option<char> = None;
        let mut cmd_args = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-0" => null_delim = true,
                "-I" => {
                    i += 1;
                    if i < args.len() {
                        replace = Some(args[i].clone());
                    }
                }
                "-n" => {
                    i += 1;
                    if i < args.len() {
                        max_args = args[i].parse().unwrap_or(0);
                    }
                }
                "-d" => {
                    i += 1;
                    if i < args.len() {
                        delim = args[i].chars().next();
                    }
                }
                _ => cmd_args.push(args[i].clone()),
            }
            i += 1;
        }
        if cmd_args.is_empty() {
            cmd_args.push("echo".to_string());
        }

        // Read all input from stdin
        let mut r = io::stdin()?;
        let max_input = proc.max_output;
        let input_str = crate::os::read_to_string_limited(&mut r, max_input)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("xargs: {e}").into()
            })?;

        let items: Vec<&str> = if null_delim {
            input_str.split('\0').filter(|s| !s.is_empty()).collect()
        } else if let Some(d) = delim {
            input_str.split(d).filter(|s| !s.is_empty()).collect()
        } else {
            input_str.split_whitespace().collect()
        };

        if items.is_empty() {
            return Ok(0);
        }

        let os_arc = io::kernel();
        let mut w = io::stdout()?;
        let mut status = 0;

        if let Some(ref repl) = replace {
            for item in &items {
                let line: String = cmd_args
                    .iter()
                    .map(|a| shell_quote(&a.replace(repl.as_str(), item)))
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut sub = exec_fork(proc);
                let (exit, _) =
                    Box::pin(crate::exec::execute(os_arc.clone(), &mut sub, &line)).await;
                w.write_all(sub.captured_output.as_bytes()).await?;
                status = exit;
            }
        } else {
            let chunks: Vec<&[&str]> = if max_args > 0 {
                items.chunks(max_args).collect()
            } else {
                vec![&items[..]]
            };
            for chunk in chunks {
                let mut parts: Vec<String> = cmd_args.iter().map(|s| shell_quote(s)).collect();
                parts.extend(chunk.iter().map(|s| shell_quote(s)));
                let line = parts.join(" ");
                let mut sub = exec_fork(proc);
                let (exit, _) =
                    Box::pin(crate::exec::execute(os_arc.clone(), &mut sub, &line)).await;
                w.write_all(sub.captured_output.as_bytes()).await?;
                status = exit;
            }
        }

        Ok(status)
    })
}
