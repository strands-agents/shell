use crate::builtins::printf::{format_float, parse_spec};
use crate::prelude::*;
#[cfg(target_arch = "wasm32")]
use std::time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::Instant;

const HELP: &str = "Usage: seq [OPTION]... LAST
  or:  seq [OPTION]... FIRST LAST
  or:  seq [OPTION]... FIRST INCREMENT LAST
Print numbers from FIRST to LAST, in steps of INCREMENT.

Options:
  -f, --format=FORMAT      use printf style floating-point FORMAT
  -s, --separator=STRING   use STRING to separate numbers (default: \\n)
  -w, --equal-width        equalize width by padding with leading zeroes";

/// Output is written in chunks of about this many bytes.
const CHUNK: usize = 8192;

/// A plain decimal operand, kept exact as `digits / 10^precision`.
struct Operand {
    digits: i128,
    precision: u32,
    /// Printed width of the operand, as GNU counts it for `-w`.
    width: i64,
}

/// Parses `[+-]DIGITS[.DIGITS]` (or `.DIGITS`), the forms GNU seq takes at face value.
fn parse_operand(arg: &str) -> Result<Operand, String> {
    let invalid = || format!("invalid floating point argument: '{arg}'");
    let text = arg.trim_start();
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (int.is_empty() && frac.is_empty()) || !all_digits(int) || !all_digits(frac) {
        return Err(invalid());
    }
    let mut digits: i128 = 0;
    for b in int.bytes().chain(frac.bytes()) {
        digits = digits
            .checked_mul(10)
            .and_then(|d| d.checked_add(i128::from(b - b'0')))
            .ok_or_else(invalid)?;
    }
    // A leading `+` is not printed, so it doesn't count toward the width.
    let mut width = (unsigned.len() + usize::from(negative)) as i64;
    if unsigned.contains('.') {
        if frac.is_empty() {
            width -= 1; // `1.` prints as `1`
        } else if int.is_empty() {
            width += 1; // `.5` prints as `0.5`
        }
    }
    Ok(Operand {
        digits: if negative { -digits } else { digits },
        precision: u32::try_from(frac.len()).map_err(|_| invalid())?,
        width,
    })
}

impl Operand {
    /// The value scaled to `10^scale`, for `scale >= self.precision`.
    fn scaled(&self, scale: u32) -> Option<i128> {
        10i128
            .checked_pow(scale - self.precision)
            .and_then(|f| self.digits.checked_mul(f))
    }
}

/// Formats `value / 10^precision` with `precision` decimals, zero-padded to `width`.
fn fixed(value: i128, precision: u32, width: usize) -> String {
    let digits = value.unsigned_abs().to_string();
    let precision = precision as usize;
    let digits = format!("{digits:0>len$}", len = precision + 1);
    let (int, frac) = digits.split_at(digits.len() - precision);
    let sign = if value < 0 { "-" } else { "" };
    let body = if precision == 0 {
        int.to_string()
    } else {
        format!("{int}.{frac}")
    };
    let pad = width.saturating_sub(sign.len() + body.len());
    format!("{sign}{}{body}", "0".repeat(pad))
}

/// A `-f` format: text around one floating-point conversion.
struct Format {
    prefix: String,
    spec: String,
    conv: char,
    suffix: String,
}

fn parse_format(fmt: &str) -> Result<Format, String> {
    let mut prefix = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            prefix.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            prefix.push('%');
            continue;
        }
        let mut spec = String::new();
        while let Some(&c) = chars.peek() {
            if "-+ #0".contains(c) || c.is_ascii_digit() || c == '.' {
                spec.push(c);
                chars.next();
            } else {
                break;
            }
        }
        // `%Lg` from C examples: the long double modifier changes nothing here.
        if chars.peek() == Some(&'L') {
            chars.next();
        }
        let conv = match chars.next() {
            Some(c @ ('e' | 'E' | 'f' | 'F' | 'g' | 'G')) => c,
            Some(c) => return Err(format!("format '{fmt}' has unknown %{c} directive")),
            None => return Err(format!("format '{fmt}' ends in %")),
        };
        let mut suffix = String::new();
        while let Some(c) = chars.next() {
            if c == '%' && chars.next() != Some('%') {
                return Err(format!("format '{fmt}' has too many % directives"));
            }
            suffix.push(c);
        }
        return Ok(Format {
            prefix,
            spec,
            conv,
            suffix,
        });
    }
    Err(format!("format '{fmt}' has no % directive"))
}

/// Whether the shell's execution deadline has passed.
fn past_deadline() -> bool {
    io::with_process(|p| p.deadline).is_some_and(|deadline| Instant::now() >= deadline)
}

#[command("seq")]
async fn cmd_seq(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let split = options_end(args);
    let mut parser = lexopt::Parser::from_args(&args[..split]);
    let mut separator = "\n".to_string();
    let mut format = None;
    let mut equal_width = false;
    let mut operands = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('s') | Long("separator") => separator = parser.value()?.string()?,
            Short('f') | Long("format") => format = Some(parser.value()?.string()?),
            Short('w') | Long("equal-width") => equal_width = true,
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => operands.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    operands.extend(args[split..].iter().cloned());
    let one = || Operand {
        digits: 1,
        precision: 0,
        width: 1,
    };
    let (first, step, last) = match operands.as_slice() {
        [] => return Err("missing operand".into()),
        [last] => (one(), one(), parse_operand(last)?),
        [first, last] => (parse_operand(first)?, one(), parse_operand(last)?),
        [first, step, last] => (
            parse_operand(first)?,
            parse_operand(step)?,
            parse_operand(last)?,
        ),
        [_, _, _, extra, ..] => return Err(format!("extra operand '{extra}'").into()),
    };
    if step.digits == 0 {
        return Err(format!("invalid Zero increment value: '{}'", operands[1]).into());
    }
    let format = match format {
        Some(_) if equal_width => {
            return Err(
                "format string may not be specified when printing equal width strings".into(),
            );
        }
        Some(f) => Some(parse_format(&f)?),
        None => None,
    };

    // GNU prints with the precision of FIRST and INCREMENT, and pads -w output
    // to the wider of FIRST and LAST at that precision.
    let precision = first.precision.max(step.precision);
    let width = if equal_width {
        let at_precision = |op: &Operand| {
            let mut w = op.width + i64::from(precision) - i64::from(op.precision);
            if op.precision == 0 && precision > 0 {
                w += 1; // room for the point
            } else if op.precision > 0 && precision == 0 {
                w -= 1; // no point printed
            }
            w
        };
        at_precision(&first).max(at_precision(&last)).max(0) as usize
    } else {
        0
    };

    let scale = precision.max(last.precision);
    let overflow = || "number out of range".to_string();
    let first_v = first.scaled(scale).ok_or_else(overflow)?;
    let step_v = step.scaled(scale).ok_or_else(overflow)?;
    let last_v = last.scaled(scale).ok_or_else(overflow)?;
    let unscale = 10i128.pow(scale - precision);

    let mut w = io::stdout()?;
    let mut out = String::new();
    let mut value = first_v;
    let mut printed = false;
    while if step_v > 0 {
        value <= last_v
    } else {
        value >= last_v
    } {
        if printed {
            out.push_str(&separator);
        }
        printed = true;
        match &format {
            Some(f) => {
                let x: f64 = fixed(value, scale, 0).parse().unwrap_or(0.0);
                out.push_str(&f.prefix);
                out.push_str(&format_float(&parse_spec(&f.spec), f.conv, x));
                out.push_str(&f.suffix);
            }
            None => out.push_str(&fixed(value / unscale, precision, width)),
        }
        if out.len() >= CHUNK {
            if past_deadline() {
                return Err("execution timeout exceeded".into());
            }
            if !write_chunk(&mut w, &out).await? {
                return Ok(1);
            }
            out.clear();
        }
        match value.checked_add(step_v) {
            Some(next) => value = next,
            None => break,
        }
    }
    if printed {
        out.push('\n');
    }
    if !write_chunk(&mut w, &out).await? {
        return Ok(1);
    }
    Ok(0)
}

/// Writes `text`, returning false when the reader has gone away.
async fn write_chunk(w: &mut crate::os::FdWriter, text: &str) -> std::io::Result<bool> {
    match w.write_all(text.as_bytes()).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// The index where options end and operands begin.
///
/// Like GNU seq, options stop at the first operand, and an argument such as
/// `-1` or `-.5` is a negative number rather than an option.
fn options_end(args: &[String]) -> usize {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let bytes = arg.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'-' || bytes[1] == b'.' || bytes[1].is_ascii_digit() {
            break;
        }
        i += 1;
        if arg == "--" {
            break;
        }
        let takes_next = if arg.starts_with("--") {
            arg == "--separator" || arg == "--format"
        } else {
            // In a cluster such as `-ws`, a trailing `s` or `f` takes the next argument.
            arg[1..]
                .find(['s', 'f'])
                .is_some_and(|pos| pos == arg.len() - 2)
        };
        if takes_next {
            i += 1;
        }
    }
    i.min(args.len())
}
