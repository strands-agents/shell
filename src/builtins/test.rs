use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{ACCESS_R, ACCESS_W, ACCESS_X, Kernel, Process};

pub fn builtin_test<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let bracket = args.last().is_some_and(|a| a == "]");
        let args = if bracket {
            &args[..args.len() - 1]
        } else {
            args
        };
        let result = match eval(args, os, proc).await {
            Ok(result) => result,
            Err(operand) => {
                let name = if bracket { "[" } else { "test" };
                proc.err_msg(&format!(
                    "strands-shell: {name}: {operand}: integer expression expected"
                ));
                return Ok(2);
            }
        };
        Ok(if result { 0 } else { 1 })
    })
}

/// A test's truth value, or the operand an integer operator could not read.
type Eval = Result<bool, String>;

/// Evaluate by POSIX's argument-count rules, so `( -n -eq )` reads `-eq` as
/// the operand of `-n` and `( -eq )` as a comparison, as bash does. Longer
/// expressions go to the parser.
fn eval<'a>(
    args: &'a [String],
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Pin<Box<dyn Future<Output = Eval> + 'a>> {
    Box::pin(async move {
        match args {
            [] => Ok(false),
            [arg] => Ok(!arg.is_empty()),
            [bang, arg] if bang == "!" => Ok(arg.is_empty()),
            [op, arg] if is_unary_op(op) => Ok(eval_unary(op, arg, os, proc).await),
            [left, op, right] if is_binary_op(op) => binary(left, op, right, os, proc).await,
            [left, op, right] if matches!(op.as_str(), "-a" | "-o") => {
                let (left, right) = (!left.is_empty(), !right.is_empty());
                Ok(if op == "-a" {
                    left && right
                } else {
                    left || right
                })
            }
            [open, arg, close] if open == "(" && close == ")" => Ok(!arg.is_empty()),
            [bang, rest @ ..] if bang == "!" && matches!(rest.len(), 2 | 3) => {
                Ok(!eval(rest, os, proc).await?)
            }
            [open, rest @ .., close] if open == "(" && close == ")" && rest.len() == 2 => {
                eval(rest, os, proc).await
            }
            _ => {
                let mut pos = 0;
                let result = parse_or(args, &mut pos, os, proc).await?;
                Ok(result && pos == args.len())
            }
        }
    })
}

/// A binary test; an integer operator rejects an operand that isn't one.
async fn binary(left: &str, op: &str, right: &str, os: &dyn Kernel, proc: &Process) -> Eval {
    if matches!(op, "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge")
        && let Some(bad) = [left, right].into_iter().find(|s| integer(s).is_none())
    {
        return Err(bad.to_string());
    }
    Ok(eval_binary(left, op, right, os, proc).await)
}

fn parse_or<'a>(
    args: &'a [String],
    pos: &'a mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Pin<Box<dyn Future<Output = Eval> + 'a>> {
    Box::pin(async move {
        let mut result = parse_and(args, pos, os, proc).await?;
        while *pos < args.len() && args[*pos] == "-o" {
            *pos += 1;
            let rhs = parse_and(args, pos, os, proc).await?;
            result = result || rhs;
        }
        Ok(result)
    })
}

fn parse_and<'a>(
    args: &'a [String],
    pos: &'a mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Pin<Box<dyn Future<Output = Eval> + 'a>> {
    Box::pin(async move {
        let mut result = parse_not(args, pos, os, proc).await?;
        while *pos < args.len() && args[*pos] == "-a" {
            *pos += 1;
            let rhs = parse_not(args, pos, os, proc).await?;
            result = result && rhs;
        }
        Ok(result)
    })
}

async fn parse_not<'a>(
    args: &'a [String],
    pos: &mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Eval {
    if *pos < args.len() && args[*pos] == "!" {
        *pos += 1;
        Ok(!parse_primary(args, pos, os, proc).await?)
    } else {
        parse_primary(args, pos, os, proc).await
    }
}

async fn parse_primary<'a>(
    args: &'a [String],
    pos: &mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Eval {
    if *pos >= args.len() {
        return Ok(false);
    }

    if args[*pos] == "(" {
        *pos += 1;
        let result = parse_or(args, pos, os, proc).await?;
        if *pos < args.len() && args[*pos] == ")" {
            *pos += 1;
        }
        return Ok(result);
    }

    if *pos + 2 < args.len()
        && let Some(next) = args.get(*pos + 1)
        && is_binary_op(next)
    {
        let left = &args[*pos];
        let op = &args[*pos + 1];
        let right = &args[*pos + 2];
        *pos += 3;
        return binary(left, op, right, os, proc).await;
    }

    if *pos + 1 < args.len() && is_unary_op(&args[*pos]) {
        let op = &args[*pos];
        let arg = &args[*pos + 1];
        *pos += 2;
        return Ok(eval_unary(op, arg, os, proc).await);
    }

    let s = &args[*pos];
    *pos += 1;
    Ok(!s.is_empty())
}

pub(crate) fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        "-n" | "-z"
            | "-e"
            | "-f"
            | "-d"
            | "-r"
            | "-w"
            | "-x"
            | "-s"
            | "-L"
            | "-h"
            | "-a"
            | "-b"
            | "-c"
            | "-p"
            | "-u"
            | "-g"
            | "-k"
            | "-t"
            | "-O"
            | "-G"
            | "-S"
    )
}

pub(crate) fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "=="
            | "!="
            | "-eq"
            | "-ne"
            | "-lt"
            | "-le"
            | "-gt"
            | "-ge"
            | "<"
            | ">"
            | "-nt"
            | "-ot"
            | "-ef"
    )
}

pub(crate) async fn eval_unary(op: &str, arg: &str, os: &dyn Kernel, proc: &Process) -> bool {
    // An empty path names no file.
    if arg.is_empty() && !matches!(op, "-n" | "-z" | "-t") {
        return false;
    }
    match op {
        "-n" => !arg.is_empty(),
        "-z" => arg.is_empty(),
        "-e" | "-a" => os.stat(proc, arg).await.exists,
        "-f" => os.stat(proc, arg).await.is_file,
        "-d" => os.stat(proc, arg).await.is_dir,
        "-s" => os.stat(proc, arg).await.len > 0,
        "-L" | "-h" => os.lstat(proc, arg).await.is_symlink,
        "-S" => os.stat(proc, arg).await.is_socket,
        "-p" => os.stat(proc, arg).await.is_fifo,
        "-b" => os.stat(proc, arg).await.is_block_device,
        "-c" => os.stat(proc, arg).await.is_char_device,
        "-r" => os.access(proc, arg, ACCESS_R).await,
        "-w" => os.access(proc, arg, ACCESS_W).await,
        "-x" => os.access(proc, arg, ACCESS_X).await,
        "-u" => os.stat(proc, arg).await.mode & 0o4000 != 0,
        "-g" => os.stat(proc, arg).await.mode & 0o2000 != 0,
        "-k" => os.stat(proc, arg).await.mode & 0o1000 != 0,
        "-t" => {
            let fd: i32 = arg.parse().unwrap_or(-1);
            os.isatty(fd)
        }
        "-O" => os.access(proc, arg, ACCESS_R).await,
        "-G" => os.access(proc, arg, ACCESS_R).await,
        _ => false,
    }
}

pub(crate) async fn eval_binary(
    left: &str,
    op: &str,
    right: &str,
    os: &dyn Kernel,
    proc: &Process,
) -> bool {
    if matches!(op, "-nt" | "-ot" | "-ef") && (left.is_empty() || right.is_empty()) {
        return op == "-nt" && !left.is_empty() && os.stat(proc, left).await.exists
            || op == "-ot" && !right.is_empty() && os.stat(proc, right).await.exists;
    }
    match op {
        "=" | "==" => left == right,
        "!=" => left != right,
        "<" => left < right,
        ">" => left > right,
        "-eq" => num(left) == num(right),
        "-ne" => num(left) != num(right),
        "-lt" => num(left) < num(right),
        "-le" => num(left) <= num(right),
        "-gt" => num(left) > num(right),
        "-ge" => num(left) >= num(right),
        "-nt" => {
            let a = os.stat(proc, left).await.modified;
            let b = os.stat(proc, right).await.modified;
            matches!((a, b), (Some(a), Some(b)) if a > b)
        }
        "-ot" => {
            let a = os.stat(proc, left).await.modified;
            let b = os.stat(proc, right).await.modified;
            matches!((a, b), (Some(a), Some(b)) if a < b)
        }
        "-ef" => {
            let a = os.stat(proc, left).await;
            let b = os.stat(proc, right).await;
            a.exists && b.exists && a.dev == b.dev && a.ino == b.ino
        }
        _ => false,
    }
}

fn num(s: &str) -> i64 {
    integer(s).unwrap_or(0)
}

/// Read an integer operand as bash does: decimal with an optional sign, any
/// whitespace before it and blanks after it.
fn integer(s: &str) -> Option<i64> {
    s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
        .trim_end_matches([' ', '\t'])
        .parse()
        .ok()
}
