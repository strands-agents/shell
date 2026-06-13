use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_printf<'a>(
    _os: &'a dyn Kernel,
    _proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.is_empty() {
            return Err("printf: usage: printf format [arguments]".into());
        }
        let fmt = &args[0];
        let params = &args[1..];
        let mut w = io::stdout()?;
        let output = format_string(fmt, params);
        w.write_all(output.as_bytes()).await?;
        Ok(0)
    })
}

fn format_string(fmt: &str, params: &[String]) -> String {
    let mut out = String::new();
    let mut pi = 0; // parameter index across all iterations

    loop {
        let mut chars = fmt.chars().peekable();
        let start_pi = pi;

        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('\\') => out.push('\\'),
                    Some('0') => {
                        // Octal
                        let mut val = 0u8;
                        for _ in 0..3 {
                            if let Some(&d) = chars.peek() {
                                if ('0'..='7').contains(&d) {
                                    val = val * 8 + (d as u8 - b'0');
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                        }
                        out.push(val as char);
                    }
                    Some(ch) => {
                        out.push('\\');
                        out.push(ch);
                    }
                    None => out.push('\\'),
                }
            } else if c == '%' {
                match chars.peek() {
                    Some('%') => {
                        chars.next();
                        out.push('%');
                    }
                    _ => {
                        // Parse format specifier: flags, width, precision, conversion
                        let mut spec = String::new();
                        // Flags
                        while let Some(&f) = chars.peek() {
                            if "-+ #0".contains(f) {
                                spec.push(f);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        // Width
                        while let Some(&d) = chars.peek() {
                            if d.is_ascii_digit() {
                                spec.push(d);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        // Precision
                        if chars.peek() == Some(&'.') {
                            spec.push('.');
                            chars.next();
                            while let Some(&d) = chars.peek() {
                                if d.is_ascii_digit() {
                                    spec.push(d);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                        }
                        let conv = chars.next().unwrap_or('s');
                        let param = params.get(pi).map(|s| s.as_str()).unwrap_or("");
                        pi += 1;
                        match conv {
                            's' => {
                                if spec.is_empty() {
                                    out.push_str(param);
                                } else {
                                    let left = spec.starts_with('-');
                                    let s = spec.trim_start_matches('-');
                                    let (width_s, prec_s) = match s.find('.') {
                                        Some(dot) => (&s[..dot], Some(&s[dot + 1..])),
                                        None => (s, None),
                                    };
                                    let width: usize = width_s.parse().unwrap_or(0);
                                    // Precision truncates to N *characters*; take
                                    // by char so a byte slice can't split a
                                    // multibyte char (e.g. `printf '%.1s' é`).
                                    let truncated;
                                    let p = match prec_s {
                                        Some(prec) => {
                                            let n = prec.parse::<usize>().unwrap_or(param.len());
                                            truncated = param.chars().take(n).collect::<String>();
                                            truncated.as_str()
                                        }
                                        None => param,
                                    };
                                    // Pad by display char count, not byte length.
                                    let p_chars = p.chars().count();
                                    if left {
                                        out.push_str(p);
                                        for _ in p_chars..width {
                                            out.push(' ');
                                        }
                                    } else {
                                        for _ in p_chars..width {
                                            out.push(' ');
                                        }
                                        out.push_str(p);
                                    }
                                }
                            }
                            'd' | 'i' => {
                                let n: i64 = param.parse().unwrap_or(0);
                                out.push_str(&n.to_string());
                            }
                            'o' => {
                                let n: i64 = param.parse().unwrap_or(0);
                                out.push_str(&format!("{:o}", n));
                            }
                            'x' => {
                                let n: i64 = param.parse().unwrap_or(0);
                                out.push_str(&format!("{:x}", n));
                            }
                            'X' => {
                                let n: i64 = param.parse().unwrap_or(0);
                                out.push_str(&format!("{:X}", n));
                            }
                            'c' => {
                                if let Some(ch) = param.chars().next() {
                                    out.push(ch);
                                }
                            }
                            'b' => {
                                // %b — interpret backslash escapes in the argument
                                out.push_str(&expand_escapes(param));
                            }
                            _ => {
                                out.push('%');
                                out.push(conv);
                            }
                        }
                    }
                }
            } else {
                out.push(c);
            }
        }
        // If no params were consumed this iteration, or all params consumed, stop
        if pi == start_pi || pi >= params.len() {
            break;
        }
    }
    out
}

fn expand_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('0') => {
                    out.push('\0');
                }
                Some(ch) => {
                    out.push('\\');
                    out.push(ch);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
