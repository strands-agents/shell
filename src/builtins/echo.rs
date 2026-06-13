use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_echo<'a>(
    _os: &'a dyn Kernel,
    _proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut start = 0;
        let mut newline = true;

        if start < args.len() && args[start] == "-n" {
            newline = false;
            start += 1;
        }

        let text = args[start..].join(" ");
        let (output, stopped) = expand_escapes(&text);

        if stopped {
            newline = false;
        }
        let mut w = io::stdout()?;
        if newline {
            wprintln!(w, "{}", output)?;
        } else {
            wprint!(w, "{}", output)?;
        }
        Ok(0)
    })
}

/// Expand escape sequences. Returns (output, hit_\c).
fn expand_escapes(s: &str) -> (String, bool) {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            None => {
                out.push('\\');
                break;
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('f') => out.push('\x0c'),
            Some('v') => out.push('\x0b'),
            Some('\\') => out.push('\\'),
            Some('0') => {
                let mut val = 0u8;
                for _ in 0..3 {
                    match chars.clone().next() {
                        Some(d @ '0'..='7') => {
                            chars.next();
                            val = val * 8 + (d as u8 - b'0');
                        }
                        _ => break,
                    }
                }
                out.push(val as char);
            }
            Some('c') => return (out, true),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    (out, false)
}
