use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_read<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut raw = false;
        let mut prompt = String::new();
        // None means the default newline delimiter.
        let mut delim: Option<u8> = None;
        let mut vars = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if a == "--" {
                vars.extend(args[i + 1..].iter().map(|s| s.as_str()));
                break;
            }
            if a.starts_with('-') && a != "-" {
                let flags: Vec<char> = a.chars().skip(1).collect();
                if flags.is_empty() || !matches!(flags[0], 'r' | 'p' | 'd') {
                    vars.push(a);
                    i += 1;
                    continue;
                }
                let mut k = 0;
                while k < flags.len() {
                    match flags[k] {
                        'r' => raw = true,
                        'p' => {
                            let attached: String = flags[k + 1..].iter().collect();
                            if !attached.is_empty() {
                                prompt = attached;
                            } else if i + 1 < args.len() {
                                i += 1;
                                prompt = args[i].clone();
                            }
                            break;
                        }
                        'd' => {
                            if k + 1 < flags.len() {
                                let attached: String = flags[k + 1..].iter().collect();
                                // An empty delimiter is NUL, as in bash.
                                delim = Some(attached.as_bytes().first().copied().unwrap_or(0));
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    proc.err_msg(
                                        "strands-shell: read: -d: option requires an argument",
                                    );
                                    return Ok(2);
                                }
                                delim = Some(args[i].as_bytes().first().copied().unwrap_or(0));
                            }
                            break;
                        }
                        c => {
                            proc.err_msg(&format!("strands-shell: read: -{c}: invalid option"));
                            return Ok(2);
                        }
                    }
                    k += 1;
                }
                i += 1;
                continue;
            }
            vars.push(a);
            i += 1;
        }
        // Bash never trims REPLY; remember whether the caller named a variable.
        let named_vars = !vars.is_empty();
        if vars.is_empty() {
            vars.push("REPLY");
        }

        if !prompt.is_empty() {
            let mut w = io::stderr()?;
            w.write_all(prompt.as_bytes()).await?;
        }

        let stop = delim.unwrap_or(b'\n');
        let mut reader = io::stdin()?;
        let mut line = String::new();
        // Read byte-by-byte to avoid BufReader consuming extra data
        let mut byte = [0u8; 1];
        let mut n = 0usize;
        let mut found_delim = false;
        loop {
            use tokio::io::AsyncReadExt;
            match reader.read(&mut byte).await {
                Ok(0) => break,
                Ok(_) => {
                    n += 1;
                    if byte[0] == stop {
                        found_delim = true;
                        break;
                    }
                    line.push(byte[0] as char);
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    let mut kind = reader.into_fd_kind();
                    if let crate::os::FdKind::ChannelReader { buf, .. } = &mut kind {
                        buf.splice(0..0, line.chars().map(|c| c as u8));
                    }
                    io::with_process(|p| p.restore_fd(0, kind));
                    return Err(e.into());
                }
                Err(_) => break,
            }
        }
        // Restore stdin so subsequent reads can use it
        io::with_process(|p| p.restore_fd(0, reader.into_fd_kind()));
        if n == 0 {
            return Ok(1); // EOF
        }
        // With a custom delimiter, bash trims only trailing IFS whitespace
        // (space, tab, newline) and only when a variable name was given.
        if delim.is_some() && named_vars {
            let ifs_chars = proc
                .env
                .get("IFS")
                .cloned()
                .unwrap_or_else(|| " \t\n".into());
            line = line
                .trim_end_matches(|c: char| matches!(c, ' ' | '\t' | '\n') && ifs_chars.contains(c))
                .to_string();
        }

        // Strip a leftover CR from CRLF, but only for the default newline delimiter.
        if delim.is_none() && line.ends_with('\r') {
            line.pop();
        }

        // Handle backslash continuation unless -r
        if !raw {
            line = line.replace("\\\n", "");
        }

        let ifs = proc
            .env
            .get("IFS")
            .cloned()
            .unwrap_or_else(|| " \t\n".into());

        if vars.len() == 1 {
            proc.set_env(vars[0], &line);
        } else {
            // Split into fields first, then assign
            let mut fields: Vec<String> = Vec::new();
            let mut rest = line.as_str();
            for vi in 0..vars.len() - 1 {
                let _ = vi;
                let trimmed = rest.trim_start_matches(|c: char| ifs.contains(c));
                if let Some(pos) = trimmed.find(|c: char| ifs.contains(c)) {
                    fields.push(trimmed[..pos].to_string());
                    rest = &trimmed[pos..];
                } else {
                    fields.push(trimmed.to_string());
                    rest = "";
                    break;
                }
            }
            // Assign fields to vars
            for (vi, var) in vars.iter().enumerate() {
                if vi == vars.len() - 1 {
                    proc.set_env(*var, rest.trim_start_matches(|c: char| ifs.contains(c)));
                } else if let Some(f) = fields.get(vi) {
                    proc.set_env(*var, f);
                } else {
                    proc.set_env(*var, "");
                }
            }
        }
        // EOF before the delimiter still assigns what was read, and returns 1.
        if delim.is_some() && !found_delim {
            Ok(1)
        } else {
            Ok(0)
        }
    })
}
