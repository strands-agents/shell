use base64::Engine;
use base64::alphabet;
use base64::engine::general_purpose::STANDARD;

use crate::os::FdWriter;
use crate::prelude::*;

const HELP: &str = "Usage: base64 [-d] [-i] [-w COLS] [FILE]
Base64 encode or decode FILE, or standard input, to standard output.
With no FILE, or when FILE is -, read standard input.

Options:
  -d, --decode          decode data
  -i, --ignore-garbage  when decoding, ignore non-alphabet characters
  -w, --wrap COLS       wrap encoded lines after COLS characters (default 76);
                        use 0 to disable line wrapping";

/// Bytes read per encode step: a multiple of 3, so only the final step
/// produces padding.
const ENCODE_CHUNK: usize = 3 * 1024;

/// Marks bytes outside the base64 alphabet in [`decode_table`].
const INVALID: u8 = 0xff;

#[command("base64")]
async fn cmd_base64(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut decode = false;
    let mut ignore_garbage = false;
    let mut wrap: usize = 76;
    let mut file = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') | Long("decode") => decode = true,
            Short('i') | Long("ignore-garbage") => ignore_garbage = true,
            Short('w') | Long("wrap") => {
                let val = parser.value()?.string()?;
                wrap = val
                    .parse()
                    .map_err(|_| format!("invalid wrap size: '{val}'"))?;
            }
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) if file.is_none() => file = Some(val.string()?),
            Value(val) => return Err(format!("extra operand '{}'", val.string()?).into()),
            _ => return Err(arg.unexpected().into()),
        }
    }

    let mut w = io::stdout()?;
    let mut ew = io::stderr()?;
    let mut reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = match file.as_deref() {
        None | Some("-") => Box::new(io::stdin()?),
        Some(path) => match io::open(os, path, OpenFlags::read()).await {
            Ok(fd) => Box::new(io::take_reader(fd)?),
            Err(e) => {
                wprintln!(ew, "base64: {}: {}", path, e)?;
                return Ok(1);
            }
        },
    };

    let result = if decode {
        decode_stream(&mut reader, &mut w, ignore_garbage).await
    } else {
        encode_stream(&mut reader, &mut w, wrap).await
    };
    match result {
        Ok(true) => Ok(0),
        Ok(false) => {
            wprintln!(ew, "base64: invalid input")?;
            Ok(1)
        }
        Err(e) => {
            wprintln!(ew, "base64: read error: {}", e)?;
            Ok(1)
        }
    }
}

/// Encodes `r` to `w`, starting a new line every `wrap` output characters
/// (never, if `wrap` is 0). Like GNU base64, a nonzero `wrap` ends non-empty
/// output with a newline, and `wrap` 0 adds none.
async fn encode_stream<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    w: &mut FdWriter,
    wrap: usize,
) -> std::io::Result<bool> {
    let mut buf = vec![0u8; ENCODE_CHUNK];
    let mut encoded = String::new();
    let mut out = Vec::new();
    let mut column = 0;
    loop {
        let n = fill(r, &mut buf).await?;
        if n == 0 {
            break;
        }
        encoded.clear();
        STANDARD.encode_string(&buf[..n], &mut encoded);
        out.clear();
        let mut rest = encoded.as_bytes();
        if wrap == 0 {
            out.extend_from_slice(rest);
        }
        while wrap > 0 && !rest.is_empty() {
            let take = rest.len().min(wrap - column);
            out.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            column += take;
            if column == wrap {
                out.push(b'\n');
                column = 0;
            }
        }
        w.write_all(&out).await?;
        if n < buf.len() {
            break;
        }
    }
    if wrap > 0 && column > 0 {
        w.write_all(b"\n").await?;
    }
    Ok(true)
}

/// Reads into `buf` until it is full or `r` reaches end of input, and returns
/// how many bytes were read.
async fn fill<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Decodes `r` to `w` the way GNU base64 does: input is read in groups of
/// four characters, newlines are skipped, and a padded group may be followed
/// by more groups, so concatenated encodings decode in sequence. With
/// `ignore_garbage`, every byte outside the alphabet and `=` is skipped.
///
/// Returns `Ok(false)` on invalid input, after writing every byte decoded
/// before the error, as GNU base64 does.
async fn decode_stream<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    w: &mut FdWriter,
    ignore_garbage: bool,
) -> std::io::Result<bool> {
    let table = decode_table();
    let mut buf = [0u8; 8192];
    let mut out = Vec::new();
    let mut group = [0u8; 4];
    let mut len = 0;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.clear();
        for &b in &buf[..n] {
            let skip = if ignore_garbage {
                b != b'=' && table[b as usize] == INVALID
            } else {
                b == b'\n'
            };
            if skip {
                continue;
            }
            group[len] = b;
            len += 1;
            if len == 4 {
                len = 0;
                if !decode_group(&table, &group, &mut out) {
                    w.write_all(&out).await?;
                    return Ok(false);
                }
            }
        }
        w.write_all(&out).await?;
    }
    if len > 0 {
        out.clear();
        decode_group(&table, &group[..len], &mut out);
        w.write_all(&out).await?;
        return Ok(false);
    }
    Ok(true)
}

/// Decodes one group of up to four characters into `out`, following gnulib's
/// `decode_4`: each byte is written as soon as the characters that make it up
/// are valid, so a bad group still yields its leading bytes. Returns false
/// unless the group is four characters of data, `xx==`, or `xxx=`. Unused low
/// bits in the last character are ignored.
fn decode_group(table: &[u8; 256], group: &[u8], out: &mut Vec<u8>) -> bool {
    let value = |i: usize| group.get(i).map_or(INVALID, |&c| table[c as usize]);
    let (a, b) = (value(0), value(1));
    if a == INVALID || b == INVALID {
        return false;
    }
    out.push(a << 2 | b >> 4);
    if group.len() < 3 {
        return false;
    }
    if group[2] == b'=' {
        return group.len() == 4 && group[3] == b'=';
    }
    let c = value(2);
    if c == INVALID {
        return false;
    }
    out.push(b << 4 | c >> 2);
    if group.len() < 4 {
        return false;
    }
    if group[3] == b'=' {
        return true;
    }
    let d = value(3);
    if d == INVALID {
        return false;
    }
    out.push(c << 6 | d);
    true
}

/// Maps each byte to its 6-bit value in the standard alphabet, or [`INVALID`].
fn decode_table() -> [u8; 256] {
    let mut table = [INVALID; 256];
    for (value, c) in alphabet::STANDARD.as_str().bytes().enumerate() {
        table[c as usize] = value as u8;
    }
    table
}
