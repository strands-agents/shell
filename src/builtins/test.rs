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
        let args = if !args.is_empty() && args[args.len() - 1] == "]" {
            &args[..args.len() - 1]
        } else {
            args
        };
        let mut pos = 0;
        let result = parse_or(args, &mut pos, os, proc).await;
        Ok(if result && pos == args.len() { 0 } else { 1 })
    })
}

fn parse_or<'a>(
    args: &'a [String],
    pos: &'a mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Pin<Box<dyn Future<Output = bool> + 'a>> {
    Box::pin(async move {
        let mut result = parse_and(args, pos, os, proc).await;
        while *pos < args.len() && args[*pos] == "-o" {
            *pos += 1;
            let rhs = parse_and(args, pos, os, proc).await;
            result = result || rhs;
        }
        result
    })
}

fn parse_and<'a>(
    args: &'a [String],
    pos: &'a mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> Pin<Box<dyn Future<Output = bool> + 'a>> {
    Box::pin(async move {
        let mut result = parse_not(args, pos, os, proc).await;
        while *pos < args.len() && args[*pos] == "-a" {
            *pos += 1;
            let rhs = parse_not(args, pos, os, proc).await;
            result = result && rhs;
        }
        result
    })
}

async fn parse_not<'a>(
    args: &'a [String],
    pos: &mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> bool {
    if *pos < args.len() && args[*pos] == "!" {
        *pos += 1;
        !parse_primary(args, pos, os, proc).await
    } else {
        parse_primary(args, pos, os, proc).await
    }
}

async fn parse_primary<'a>(
    args: &'a [String],
    pos: &mut usize,
    os: &'a dyn Kernel,
    proc: &'a Process,
) -> bool {
    if *pos >= args.len() {
        return false;
    }

    if args[*pos] == "(" {
        *pos += 1;
        let result = parse_or(args, pos, os, proc).await;
        if *pos < args.len() && args[*pos] == ")" {
            *pos += 1;
        }
        return result;
    }

    if *pos + 2 <= args.len()
        && let Some(next) = args.get(*pos + 1)
        && is_binary_op(next)
    {
        let left = &args[*pos];
        let op = &args[*pos + 1];
        let right = &args[*pos + 2];
        *pos += 3;
        return eval_binary(left, op, right, os, proc).await;
    }

    if *pos + 1 < args.len() && is_unary_op(&args[*pos]) {
        let op = &args[*pos];
        let arg = &args[*pos + 1];
        *pos += 2;
        return eval_unary(op, arg, os, proc).await;
    }

    let s = &args[*pos];
    *pos += 1;
    !s.is_empty()
}

fn is_unary_op(s: &str) -> bool {
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

fn is_binary_op(s: &str) -> bool {
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

async fn eval_unary(op: &str, arg: &str, os: &dyn Kernel, proc: &Process) -> bool {
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

async fn eval_binary(left: &str, op: &str, right: &str, os: &dyn Kernel, proc: &Process) -> bool {
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
    s.parse().unwrap_or(0)
}
