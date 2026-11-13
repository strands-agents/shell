use std::future::Future;
use std::iter::Peekable;
use std::pin::Pin;
use std::str::Chars;

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
        w.write_all(&output).await?;
        Ok(0)
    })
}

/// printf output: raw bytes, since `\xHH` and octal escapes can emit
/// bytes that are not valid UTF-8.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn push(&mut self, c: char) {
        self.0
            .extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
    }
    fn push_str(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
    }
}

fn format_string(fmt: &str, params: &[String]) -> Vec<u8> {
    let mut out = Out::default();
    let mut pi = 0; // parameter index across all iterations

    loop {
        let mut chars = fmt.chars().peekable();
        let start_pi = pi;

        while let Some(c) = chars.next() {
            if c == '\\' {
                escape(&mut chars, false, &mut out);
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
                                out.push_str(&format_signed(&parse_spec(&spec), n));
                            }
                            'u' | 'o' | 'x' | 'X' => {
                                let n: i64 = param.parse().unwrap_or(0);
                                out.push_str(&format_unsigned(&parse_spec(&spec), conv, n as u64));
                            }
                            'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                                let x: f64 = param.trim().parse().unwrap_or(0.0);
                                out.push_str(&format_float(&parse_spec(&spec), conv, x));
                            }
                            'c' => {
                                if let Some(ch) = param.chars().next() {
                                    out.push(ch);
                                }
                            }
                            'b' => {
                                // %b — interpret backslash escapes in the argument;
                                // `\c` stops all further output.
                                if escape_all(param, &mut out) {
                                    return out.0;
                                }
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
    out.0
}

/// Largest width or precision honoured; larger values are clamped so a huge
/// field can't exhaust memory (Rust's formatter also rejects precisions above
/// `u16::MAX`).
const MAX_FIELD: usize = u16::MAX as usize;

/// Flags, width and precision of one numeric conversion.
#[derive(Default)]
pub(crate) struct Spec {
    left: bool,
    zero: bool,
    plus: bool,
    space: bool,
    alt: bool,
    width: usize,
    precision: Option<usize>,
}

pub(crate) fn parse_spec(spec: &str) -> Spec {
    let mut out = Spec::default();
    let rest = spec.trim_start_matches(|c| {
        match c {
            '-' => out.left = true,
            '0' => out.zero = true,
            '+' => out.plus = true,
            ' ' => out.space = true,
            '#' => out.alt = true,
            _ => return false,
        }
        true
    });
    // Digits only, so a failed parse means overflow: clamp it too.
    let field = |s: &str| s.parse().map_or(MAX_FIELD, |n: usize| n.min(MAX_FIELD));
    let (width, precision) = match rest.split_once('.') {
        Some((w, p)) => (w, Some(if p.is_empty() { 0 } else { field(p) })),
        None => (rest, None),
    };
    out.width = if width.is_empty() { 0 } else { field(width) };
    out.precision = precision;
    out
}

/// Pads `prefix` + `body` to the field width: spaces on the right for `-`,
/// zeros after the prefix for `0` (when `zero_ok`), spaces on the left otherwise.
fn pad(spec: &Spec, prefix: &str, body: &str, zero_ok: bool) -> String {
    let fill = spec.width.saturating_sub(prefix.len() + body.len());
    if spec.left {
        format!("{prefix}{body}{}", " ".repeat(fill))
    } else if spec.zero && zero_ok {
        format!("{prefix}{}{body}", "0".repeat(fill))
    } else {
        format!("{}{prefix}{body}", " ".repeat(fill))
    }
}

/// Applies an integer precision: the minimum number of digits, where a zero
/// value with precision 0 prints no digits at all.
fn int_digits(digits: String, spec: &Spec) -> String {
    match spec.precision {
        Some(0) if digits == "0" => String::new(),
        Some(p) if digits.len() < p => format!("{}{digits}", "0".repeat(p - digits.len())),
        _ => digits,
    }
}

/// `%d`/`%i`: a sign from `-`, `+` or space, then the digits.
fn format_signed(spec: &Spec, n: i64) -> String {
    let sign = if n < 0 {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    let body = int_digits(n.unsigned_abs().to_string(), spec);
    pad(spec, sign, &body, spec.precision.is_none())
}

/// `%u`/`%o`/`%x`/`%X`; `#` gives octal a leading 0 and nonzero hex a `0x`.
fn format_unsigned(spec: &Spec, conv: char, n: u64) -> String {
    let digits = match conv {
        'u' => n.to_string(),
        'o' => format!("{n:o}"),
        'x' => format!("{n:x}"),
        _ => format!("{n:X}"),
    };
    let mut body = int_digits(digits, spec);
    let mut prefix = "";
    if spec.alt && conv == 'o' && !body.starts_with('0') {
        body.insert(0, '0');
    } else if spec.alt && n != 0 && conv == 'x' {
        prefix = "0x";
    } else if spec.alt && n != 0 && conv == 'X' {
        prefix = "0X";
    }
    pad(spec, prefix, &body, spec.precision.is_none())
}

/// `%f`/`%e`/`%g` and their uppercase forms.
pub(crate) fn format_float(spec: &Spec, conv: char, x: f64) -> String {
    let sign = if x.is_sign_negative() {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    let a = x.abs();
    let p = spec.precision.unwrap_or(6);
    let body = if a.is_nan() {
        "nan".to_string()
    } else if a.is_infinite() {
        "inf".to_string()
    } else {
        match conv.to_ascii_lowercase() {
            'f' => fixed(a, p, spec.alt),
            'e' => exponent(a, p, spec.alt),
            _ => general(a, p, spec.alt),
        }
    };
    let body = if conv.is_ascii_uppercase() {
        body.to_ascii_uppercase()
    } else {
        body
    };
    pad(spec, sign, &body, x.is_finite())
}

/// `%f`: `p` digits after the point; `#` keeps the point when `p` is 0.
fn fixed(a: f64, p: usize, alt: bool) -> String {
    let mut s = format!("{a:.p$}");
    if alt && p == 0 {
        s.push('.');
    }
    s
}

/// `%e`: one digit, `p` decimals, and a signed exponent of at least two digits.
fn exponent(a: f64, p: usize, alt: bool) -> String {
    let s = format!("{a:.p$e}");
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let dot = if alt && p == 0 { "." } else { "" };
    let exp_sign = if exp < 0 { '-' } else { '+' };
    format!("{mantissa}{dot}e{exp_sign}{:02}", exp.unsigned_abs())
}

/// `%g`: `%e` when the exponent is below -4 or at least the precision, `%f`
/// otherwise, with `p` significant digits and trailing zeros removed unless `#`.
fn general(a: f64, p: usize, alt: bool) -> String {
    // The `%f` form uses up to p + 3 decimals (exponent -4); keep that in range.
    let p = p.clamp(1, MAX_FIELD - 3);
    let exp: i32 = if a == 0.0 {
        0
    } else {
        let s = format!("{a:.*e}", p - 1);
        s.split_once('e')
            .and_then(|(_, e)| e.parse().ok())
            .unwrap_or(0)
    };
    let s = if exp >= -4 && i64::from(exp) < p as i64 {
        fixed(a, (p as i64 - 1 - i64::from(exp)) as usize, alt)
    } else {
        exponent(a, p - 1, alt)
    };
    if alt {
        return s;
    }
    let (mantissa, exp) = s.split_at(s.find('e').unwrap_or(s.len()));
    let mantissa = if mantissa.contains('.') {
        mantissa.trim_end_matches('0').trim_end_matches('.')
    } else {
        mantissa
    };
    format!("{mantissa}{exp}")
}

/// Expand every backslash escape in a `%b` argument. Returns true on `\c`.
fn escape_all(s: &str, out: &mut Out) -> bool {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
        } else if escape(&mut chars, true, out) {
            return true;
        }
    }
    false
}

/// Interpret one escape, `chars` positioned just after the backslash, the
/// way bash's printf does. `in_b` selects `%b` rules: `\0` takes up to
/// three more octal digits and `\c` stops output (returns true).
fn escape(chars: &mut Peekable<Chars<'_>>, in_b: bool, out: &mut Out) -> bool {
    let Some(c) = chars.next() else {
        out.push('\\');
        return false;
    };
    match c {
        'n' => out.push('\n'),
        't' => out.push('\t'),
        'r' => out.push('\r'),
        '\\' => out.push('\\'),
        'c' if in_b => return true,
        '0'..='7' => {
            let mut val = c as u32 - '0' as u32;
            let more = if in_b && c == '0' { 3 } else { 2 };
            for _ in 0..more {
                match chars.peek().and_then(|d| d.to_digit(8)) {
                    Some(d) => {
                        val = val * 8 + d;
                        chars.next();
                    }
                    None => break,
                }
            }
            out.0.push(val as u8); // bash keeps the low byte: `\400` is NUL
        }
        'x' | 'u' | 'U' => {
            let max = match c {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let mut val = 0u32;
            let mut n = 0;
            while n < max {
                match chars.peek().and_then(|d| d.to_digit(16)) {
                    Some(d) => {
                        val = val * 16 + d;
                        chars.next();
                        n += 1;
                    }
                    None => break,
                }
            }
            if n == 0 {
                out.push('\\');
                out.push(c);
            } else if c == 'x' {
                out.0.push(val as u8);
            } else if let Some(ch) = char::from_u32(val) {
                // Unlike bash, surrogates and values past U+10FFFF are
                // skipped rather than encoded as invalid UTF-8.
                out.push(ch);
            }
        }
        _ => {
            out.push('\\');
            out.push(c);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::format_string;

    fn fmt(f: &str, args: &[&str]) -> Vec<u8> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        format_string(f, &args)
    }

    #[test]
    fn escapes_emit_raw_bytes() {
        assert_eq!(fmt(r"\xff\200\x00\400\777", &[]), b"\xff\x80\0\0\xff");
        assert_eq!(fmt("%b", &[r"\xff\0200\0"]), b"\xff\x80\0");
    }

    #[test]
    fn unicode_escapes_skip_invalid_code_points() {
        assert_eq!(fmt(r"\u0041\uD800\U110000\u00e9", &[]), "Aé".as_bytes());
    }
}
