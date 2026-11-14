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
            match args[i].as_str() {
                "-r" => raw = true,
                "-p" => {
                    i += 1;
                    if i < args.len() {
                        prompt = args[i].clone();
                    }
                }
                "-d" => {
                    i += 1;
                    if i >= args.len() {
                        proc.err_msg("strands-shell: read: -d: option requires an argument");
                        return Ok(2);
                    }
                    // An empty delimiter is NUL, as in bash.
                    delim = Some(args[i].as_bytes().first().copied().unwrap_or(0));
                }
                _ => vars.push(args[i].as_str()),
            }
            i += 1;
        }
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
        // A custom delimiter: bash trims trailing IFS from the record, so
        // `read -d ''` of `a\nb\n` with the default IFS yields `a\nb`.
        if delim.is_some() {
            let ifs_chars = proc
                .env
                .get("IFS")
                .cloned()
                .unwrap_or_else(|| " \t\n".into());
            line = line
                .trim_end_matches(|c: char| ifs_chars.contains(c))
                .to_string();
        }

        // Strip trailing newline
        if line.ends_with('\r') {
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
