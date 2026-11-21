use std::future::Future;
use std::pin::Pin;

use super::printf::{Escapes, Out, escape_all};
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
        let mut escapes = Some(Escapes::PlainEcho);

        // Like bash, leading words made only of the letters n, e and E are
        // options; anything else (e.g. `-x`, `--`, `-`) is printed literally.
        while let Some(flags) = args.get(start).and_then(|a| a.strip_prefix('-')) {
            if flags.is_empty() || !flags.chars().all(|c| matches!(c, 'n' | 'e' | 'E')) {
                break;
            }
            for c in flags.chars() {
                match c {
                    'n' => newline = false,
                    'e' => escapes = Some(Escapes::Echo),
                    _ => escapes = None,
                }
            }
            start += 1;
        }

        let text = args[start..].join(" ");
        let mut out = Out::default();
        if let Some(mode) = escapes {
            if escape_all(&text, mode, &mut out) {
                newline = false;
            }
        } else {
            out.push_str(&text);
        }
        if newline {
            out.push('\n');
        }
        io::stdout()?.write_all(&out.0).await?;
        Ok(0)
    })
}
