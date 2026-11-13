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

/// GNU xargs stops when the command exits 255 or could not be run (126/127).
/// The in-process status can't tell "could not be run" apart from a command
/// that itself exits 126/127, so both stop xargs.
fn stops(exit: i32) -> bool {
    matches!(exit, 126 | 127 | 255)
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
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            if arg == "--" {
                i += 1;
                break;
            }
            if arg == "-0" {
                null_delim = true;
                i += 1;
                continue;
            }
            let opt = match arg.get(..2) {
                Some(o @ ("-I" | "-n" | "-d")) => o,
                _ => break,
            };
            let value = if arg.len() > 2 {
                Some(arg[2..].to_string())
            } else {
                i += 1;
                args.get(i).cloned()
            };
            if let Some(v) = value {
                match opt {
                    "-I" => replace = Some(v),
                    "-n" => max_args = v.parse().unwrap_or(0),
                    _ => delim = v.chars().next(),
                }
            }
            i += 1;
        }
        let mut cmd_args: Vec<String> = args[i.min(args.len())..].to_vec();
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
        let mut ew = io::stderr().ok();
        let mut status = 0;
        let mut failed = false;

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
                if let Some(ew) = ew.as_mut() {
                    ew.write_all(sub.captured_stderr.as_bytes()).await?;
                }
                status = exit;
                if stops(status) {
                    break;
                }
                failed |= status != 0;
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
                if let Some(ew) = ew.as_mut() {
                    ew.write_all(sub.captured_stderr.as_bytes()).await?;
                }
                status = exit;
                if stops(status) {
                    break;
                }
                failed |= status != 0;
            }
        }

        if proc.check_limits().is_some() {
            return Ok(1);
        }
        // GNU: 124 after a 255, 126/127 if the command could not be run,
        // otherwise 123 if any invocation failed.
        Ok(match status {
            255 => {
                proc.err_msg(&format!(
                    "xargs: {}: exited with status 255; aborting",
                    cmd_args[0]
                ));
                124
            }
            126 | 127 => status,
            _ if failed => 123,
            _ => 0,
        })
    })
}
