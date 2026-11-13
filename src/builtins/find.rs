use std::future::Future;
use std::pin::Pin;

use tokio::io::AsyncWriteExt;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

fn exec_fork(proc: &mut Process) -> Process {
    let mut sub = proc.fork();
    sub.depth += 1;
    sub.capture = true;
    sub
}

fn glob_match(pat: &[u8], val: &[u8]) -> bool {
    let (mut pi, mut vi) = (0, 0);
    let (mut star_p, mut star_v) = (usize::MAX, 0);
    while vi < val.len() {
        if pi < pat.len() && pat[pi] == b'*' {
            star_p = pi;
            star_v = vi;
            pi += 1;
        } else if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == val[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < pat.len() && pat[pi] == b'[' {
            let start = pi + 1;
            let negated = start < pat.len() && (pat[start] == b'!' || pat[start] == b'^');
            let mut ci = if negated { start + 1 } else { start };
            let mut matched = false;
            while ci < pat.len() && pat[ci] != b']' {
                if ci + 2 < pat.len() && pat[ci + 1] == b'-' {
                    if val[vi] >= pat[ci] && val[vi] <= pat[ci + 2] {
                        matched = true;
                    }
                    ci += 3;
                } else {
                    if val[vi] == pat[ci] {
                        matched = true;
                    }
                    ci += 1;
                }
            }
            if negated {
                matched = !matched;
            }
            if matched {
                pi = if ci < pat.len() { ci + 1 } else { ci };
                vi += 1;
            } else if star_p != usize::MAX {
                pi = star_p + 1;
                star_v += 1;
                vi = star_v;
            } else {
                return false;
            }
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_v += 1;
            vi = star_v;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

fn glob_match_icase(pat: &str, val: &str) -> bool {
    glob_match(pat.to_lowercase().as_bytes(), val.to_lowercase().as_bytes())
}

#[derive(Clone)]
enum Expr {
    Name(String),
    IName(String),
    Path(String),
    Type(char),
    Empty,
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Print,
    Print0,
    Exec(Vec<String>, bool), // (cmd_template, plus_mode)
    Delete,
    True,
    /// `-size`: comparison, count, unit in bytes.
    Size(Cmp, u64, u64),
    /// `-mtime`/`-mmin`: comparison, reference age and window in seconds,
    /// and the current time, filled in by `resolve_times`.
    Age(Cmp, f64, f64, Option<std::time::SystemTime>),
    /// `-newer FILE`: FILE and its mtime, filled in by `resolve_times`.
    Newer(String, Option<std::time::SystemTime>),
}

#[derive(Clone, Copy)]
enum Cmp {
    Lt,
    Eq,
    Gt,
}

/// Split a leading `+` (greater than) or `-` (less than) off a numeric argument.
fn split_cmp(s: &str) -> (Cmp, &str) {
    if let Some(rest) = s.strip_prefix('+') {
        (Cmp::Gt, rest)
    } else if let Some(rest) = s.strip_prefix('-') {
        (Cmp::Lt, rest)
    } else {
        (Cmp::Eq, s)
    }
}

/// Parse `-size [+-]N[bcwkMG]`, which counts the size in units rounded up.
fn parse_size(arg: &str) -> Result<Expr, String> {
    let Some(last) = arg.chars().last() else {
        return Err("find: invalid null argument to -size".into());
    };
    let (num, unit) = match last {
        'b' => (&arg[..arg.len() - 1], 512),
        'c' => (&arg[..arg.len() - 1], 1),
        'w' => (&arg[..arg.len() - 1], 2),
        'k' => (&arg[..arg.len() - 1], 1024),
        'M' => (&arg[..arg.len() - 1], 1024 * 1024),
        'G' => (&arg[..arg.len() - 1], 1024 * 1024 * 1024),
        c if c.is_ascii_digit() => (arg, 512),
        c => return Err(format!("find: invalid -size type `{c}'")),
    };
    let (cmp, digits) = split_cmp(num);
    match digits.parse::<u64>() {
        Ok(n) if digits.bytes().all(|b| b.is_ascii_digit()) => Ok(Expr::Size(cmp, n, unit)),
        _ => Err(format!("find: Invalid argument `{arg}' to -size")),
    }
}

/// Parse the `[+-]N` of `-mtime` (`unit` 86400) or `-mmin` (`unit` 60).
fn parse_age(pred: &str, arg: &str, unit: f64) -> Result<Expr, String> {
    let (cmp, num) = split_cmp(arg);
    let valid = num.bytes().any(|b| b.is_ascii_digit())
        && num.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && num.bytes().filter(|&b| b == b'.').count() <= 1;
    let n: f64 = match num.parse() {
        Ok(n) if valid => n,
        _ => return Err(format!("find: invalid argument `{arg}' to `{pred}'")),
    };
    // GNU counts -mtime N and +N from the N+1th day back, and -mtime -N
    // from one second past the Nth. -mmin has no such shift.
    let base = match cmp {
        _ if pred != "-mtime" => n * unit,
        Cmp::Lt => n * unit + 1.0,
        _ => (n + 1.0) * unit,
    };
    Ok(Expr::Age(cmp, base, unit, None))
}

/// Fill in the current time for `-mtime`/`-mmin` and the mtime of each
/// `-newer FILE` (without following a symlink).
fn resolve_times<'a>(
    os: &'a dyn Kernel,
    expr: &'a mut Expr,
    now: std::time::SystemTime,
) -> Pin<Box<dyn Future<Output = Result<(), String>> + 'a>> {
    Box::pin(async move {
        match expr {
            Expr::Age(.., at) => *at = Some(now),
            Expr::Newer(file, reference) => {
                let st = io::lstat(os, file).await;
                if !st.exists {
                    return Err(format!("find: '{file}': No such file or directory"));
                }
                *reference = Some(st.modified.unwrap_or(std::time::UNIX_EPOCH));
            }
            Expr::Not(e) => resolve_times(os, e, now).await?,
            Expr::And(a, b) | Expr::Or(a, b) => {
                resolve_times(os, a, now).await?;
                resolve_times(os, b, now).await?;
            }
            _ => {}
        }
        Ok(())
    })
}

fn parse_expr(args: &[String], pos: &mut usize) -> Result<Expr, String> {
    parse_or(args, pos)
}

fn parse_or(args: &[String], pos: &mut usize) -> Result<Expr, String> {
    let mut left = parse_and(args, pos)?;
    while *pos < args.len() && args[*pos] == "-o" {
        *pos += 1;
        let right = parse_and(args, pos)?;
        left = Expr::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_and(args: &[String], pos: &mut usize) -> Result<Expr, String> {
    let mut left = parse_unary(args, pos)?;
    loop {
        if *pos < args.len() && args[*pos] == "-a" {
            *pos += 1;
        }
        if *pos >= args.len() {
            break;
        }
        let next = &args[*pos];
        if next == "-o" || next == ")" {
            break;
        }
        let right = parse_unary(args, pos)?;
        left = Expr::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_unary(args: &[String], pos: &mut usize) -> Result<Expr, String> {
    if *pos >= args.len() {
        return Ok(Expr::True);
    }
    if args[*pos] == "!" || args[*pos] == "-not" {
        *pos += 1;
        let inner = parse_unary(args, pos)?;
        return Ok(Expr::Not(Box::new(inner)));
    }
    if args[*pos] == "(" {
        *pos += 1;
        let inner = parse_or(args, pos)?;
        if *pos < args.len() && args[*pos] == ")" {
            *pos += 1;
        }
        return Ok(inner);
    }
    parse_primary(args, pos)
}

fn parse_primary(args: &[String], pos: &mut usize) -> Result<Expr, String> {
    if *pos >= args.len() {
        return Ok(Expr::True);
    }
    let tok = &args[*pos];
    match tok.as_str() {
        "-name" => {
            *pos += 1;
            need_arg(args, pos).map(Expr::Name)
        }
        "-iname" => {
            *pos += 1;
            need_arg(args, pos).map(Expr::IName)
        }
        "-path" | "-wholename" => {
            *pos += 1;
            need_arg(args, pos).map(Expr::Path)
        }
        "-type" => {
            *pos += 1;
            let s = need_arg(args, pos)?;
            Ok(Expr::Type(s.chars().next().unwrap_or('f')))
        }
        "-empty" => {
            *pos += 1;
            Ok(Expr::Empty)
        }
        "-print" => {
            *pos += 1;
            Ok(Expr::Print)
        }
        "-print0" => {
            *pos += 1;
            Ok(Expr::Print0)
        }
        "-delete" => {
            *pos += 1;
            Ok(Expr::Delete)
        }
        "-size" => {
            *pos += 1;
            parse_size(&need_arg(args, pos)?)
        }
        "-mtime" => {
            *pos += 1;
            parse_age(tok, &need_arg(args, pos)?, 86400.0)
        }
        "-mmin" => {
            *pos += 1;
            parse_age(tok, &need_arg(args, pos)?, 60.0)
        }
        "-newer" => {
            *pos += 1;
            need_arg(args, pos).map(|f| Expr::Newer(f, None))
        }
        "-exec" => {
            *pos += 1;
            let mut cmd = Vec::new();
            let mut plus = false;
            while *pos < args.len() {
                if args[*pos] == ";" {
                    *pos += 1;
                    break;
                }
                if args[*pos] == "+" {
                    *pos += 1;
                    plus = true;
                    break;
                }
                cmd.push(args[*pos].clone());
                *pos += 1;
            }
            Ok(Expr::Exec(cmd, plus))
        }
        _ => Err(format!("find: unknown predicate: '{tok}'")),
    }
}

fn need_arg(args: &[String], pos: &mut usize) -> Result<String, String> {
    if *pos >= args.len() {
        return Err("find: missing argument".into());
    }
    let v = args[*pos].clone();
    *pos += 1;
    Ok(v)
}

fn eval_expr(
    expr: &Expr,
    path: &str,
    name: &str,
    st: &crate::os::FileStat,
    is_empty_dir: bool,
    delete_hit: &mut bool,
) -> (bool, bool) {
    // Returns (matched, has_action), and sets `delete_hit` when evaluation
    // reaches -delete.
    match expr {
        Expr::True => (true, false),
        Expr::Name(pat) => (glob_match(pat.as_bytes(), name.as_bytes()), false),
        Expr::IName(pat) => (glob_match_icase(pat, name), false),
        Expr::Path(pat) => (glob_match(pat.as_bytes(), path.as_bytes()), false),
        Expr::Type(c) => {
            let m = match c {
                'f' => st.is_file,
                'd' => st.is_dir,
                'l' => st.is_symlink,
                'p' => st.is_fifo,
                's' => st.is_socket,
                _ => false,
            };
            (m, false)
        }
        Expr::Empty => {
            let m = if st.is_file {
                st.len == 0
            } else if st.is_dir {
                is_empty_dir
            } else {
                false
            };
            (m, false)
        }
        Expr::Not(inner) => {
            let (m, p) = eval_expr(inner, path, name, st, is_empty_dir, delete_hit);
            (!m, p)
        }
        Expr::And(a, b) => {
            let (ma, pa) = eval_expr(a, path, name, st, is_empty_dir, delete_hit);
            if !ma {
                return (false, pa);
            }
            let (mb, pb) = eval_expr(b, path, name, st, is_empty_dir, delete_hit);
            (mb, pa || pb)
        }
        Expr::Or(a, b) => {
            let (ma, pa) = eval_expr(a, path, name, st, is_empty_dir, delete_hit);
            if ma {
                return (true, pa);
            }
            let (mb, pb) = eval_expr(b, path, name, st, is_empty_dir, delete_hit);
            (mb, pa || pb)
        }
        Expr::Size(cmp, n, unit) => {
            let units = st.len.div_ceil(*unit);
            let m = match cmp {
                Cmp::Lt => units < *n,
                Cmp::Eq => units == *n,
                Cmp::Gt => units > *n,
            };
            (m, false)
        }
        Expr::Age(cmp, base, window, now) => {
            let m = matches!((st.modified, now), (Some(mtime), Some(now)) if {
                let age = match now.duration_since(mtime) {
                    Ok(d) => d.as_secs_f64(),
                    Err(e) => -e.duration().as_secs_f64(),
                };
                match cmp {
                    Cmp::Lt => age < *base,
                    Cmp::Eq => age > *base - *window && age <= *base,
                    Cmp::Gt => age > *base,
                }
            });
            (m, false)
        }
        Expr::Newer(_, reference) => {
            let m = matches!((st.modified, reference), (Some(t), Some(r)) if t > *r);
            (m, false)
        }
        Expr::Print | Expr::Print0 | Expr::Exec(..) => (true, true),
        Expr::Delete => {
            *delete_hit = true;
            (true, true)
        }
    }
}

/// Whether `expr` holds a node for which `want` is true.
fn contains(expr: &Expr, want: fn(&Expr) -> bool) -> bool {
    want(expr)
        || match expr {
            Expr::Not(e) => contains(e, want),
            Expr::And(a, b) | Expr::Or(a, b) => contains(a, want) || contains(b, want),
            _ => false,
        }
}

fn has_action(expr: &Expr) -> bool {
    match expr {
        Expr::Print | Expr::Print0 | Expr::Exec(..) | Expr::Delete => true,
        Expr::Not(e) => has_action(e),
        Expr::And(a, b) | Expr::Or(a, b) => has_action(a) || has_action(b),
        _ => false,
    }
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./-_.".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn builtin_find<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut paths: Vec<String> = Vec::new();
        let mut max_depth: Option<usize> = None;
        let mut min_depth: usize = 0;
        let mut expr_args: Vec<String> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            match args[i].as_str() {
                "-maxdepth" => {
                    i += 1;
                    if i < args.len() {
                        max_depth = args[i].parse().ok();
                    }
                    i += 1;
                }
                "-mindepth" => {
                    i += 1;
                    if i < args.len() {
                        min_depth = args[i].parse().unwrap_or(0);
                    }
                    i += 1;
                }
                s if !s.starts_with('-') && s != "!" && s != "(" && expr_args.is_empty() => {
                    paths.push(args[i].clone());
                    i += 1;
                }
                _ => {
                    expr_args = args[i..].to_vec();
                    break;
                }
            }
        }

        // Also strip -maxdepth/-mindepth from expr_args
        let mut filtered = Vec::new();
        let mut j = 0;
        while j < expr_args.len() {
            match expr_args[j].as_str() {
                "-maxdepth" => {
                    j += 1;
                    if j < expr_args.len() {
                        max_depth = expr_args[j].parse().ok();
                    }
                    j += 1;
                }
                "-mindepth" => {
                    j += 1;
                    if j < expr_args.len() {
                        min_depth = expr_args[j].parse().unwrap_or(0);
                    }
                    j += 1;
                }
                _ => {
                    filtered.push(expr_args[j].clone());
                    j += 1;
                }
            }
        }

        if paths.is_empty() {
            paths.push(".".into());
        }

        let mut pos = 0;
        let mut expr = match parse_expr(&filtered, &mut pos) {
            Ok(e) => e,
            Err(e) => {
                proc.err_msg(&e);
                return Ok(1);
            }
        };
        if let Err(e) = resolve_times(os, &mut expr, os.now()).await {
            proc.err_msg(&e);
            return Ok(1);
        }

        let default_print = !has_action(&expr);
        // -delete implies -depth: a directory is visited after its entries.
        let depth_first = contains(&expr, |e| matches!(e, Expr::Delete));
        // `-exec CMD ;` counts as true whatever CMD returns, so a -delete
        // after it would remove every path the walk reaches.
        if depth_first && contains(&expr, |e| matches!(e, Expr::Exec(_, false))) {
            proc.err_msg("find: -delete cannot be combined with -exec ... ;");
            return Ok(1);
        }
        let mut failed = false;
        let mut w = io::stdout()?;
        let mut ew = io::stderr().ok();
        let mut exec_plus_matches: Vec<String> = Vec::new();
        let mut exec_plus_cmd: Option<Vec<String>> = None;

        for start_path in &paths {
            walk(
                os,
                proc,
                &mut w,
                &mut ew,
                start_path,
                &expr,
                max_depth,
                min_depth,
                0,
                default_print,
                depth_first,
                &mut exec_plus_matches,
                &mut exec_plus_cmd,
                &mut failed,
            )
            .await?;
        }

        if let Some(cmd_template) = &exec_plus_cmd
            && !exec_plus_matches.is_empty()
        {
            let mut parts: Vec<String> = Vec::new();
            for a in cmd_template {
                if a == "{}" {
                    parts.extend(exec_plus_matches.iter().cloned());
                } else {
                    parts.push(a.clone());
                }
            }
            let line = parts
                .iter()
                .map(|s| shell_quote(s))
                .collect::<Vec<_>>()
                .join(" ");
            let os_arc = io::kernel();
            let mut sub = exec_fork(proc);
            let _ = Box::pin(crate::exec::execute(os_arc, &mut sub, &line)).await;
            w.write_all(sub.captured_output.as_bytes()).await?;
            if let Some(ew) = ew.as_mut() {
                ew.write_all(sub.captured_stderr.as_bytes()).await?;
            }
        }

        Ok(if failed { 1 } else { 0 })
    })
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    w: &'a mut crate::os::FdWriter,
    ew: &'a mut Option<crate::os::FdWriter>,
    path: &'a str,
    expr: &'a Expr,
    max_depth: Option<usize>,
    min_depth: usize,
    depth: usize,
    default_print: bool,
    depth_first: bool,
    exec_plus_matches: &'a mut Vec<String>,
    exec_plus_cmd: &'a mut Option<Vec<String>>,
    failed: &'a mut bool,
) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + 'a>> {
    Box::pin(async move {
        let st = io::lstat(os, path).await;
        if !st.exists {
            return Ok(());
        }
        let visit_here = depth >= min_depth;
        if visit_here && !depth_first {
            visit(
                os,
                proc,
                w,
                ew,
                path,
                &st,
                expr,
                default_print,
                exec_plus_matches,
                exec_plus_cmd,
                failed,
            )
            .await?;
        }

        let at_max = matches!(max_depth, Some(max) if depth >= max);
        if st.is_dir
            && !at_max
            && let Ok(entries) = io::list_dir(os, path).await
        {
            for entry in &entries {
                let child = if path == "." {
                    format!("./{}", entry.name)
                } else if path.ends_with('/') {
                    format!("{path}{}", entry.name)
                } else {
                    format!("{path}/{}", entry.name)
                };
                walk(
                    os,
                    proc,
                    w,
                    ew,
                    &child,
                    expr,
                    max_depth,
                    min_depth,
                    depth + 1,
                    default_print,
                    depth_first,
                    exec_plus_matches,
                    exec_plus_cmd,
                    failed,
                )
                .await?;
            }
        }

        if visit_here && depth_first {
            visit(
                os,
                proc,
                w,
                ew,
                path,
                &st,
                expr,
                default_print,
                exec_plus_matches,
                exec_plus_cmd,
                failed,
            )
            .await?;
        }
        Ok(())
    })
}

/// Evaluate `expr` for `path`, run the actions it reaches, and remove the
/// path if it reaches -delete.
#[allow(clippy::too_many_arguments)]
async fn visit(
    os: &dyn Kernel,
    proc: &mut Process,
    w: &mut crate::os::FdWriter,
    ew: &mut Option<crate::os::FdWriter>,
    path: &str,
    st: &crate::os::FileStat,
    expr: &Expr,
    default_print: bool,
    exec_plus_matches: &mut Vec<String>,
    exec_plus_cmd: &mut Option<Vec<String>>,
    failed: &mut bool,
) -> std::io::Result<()> {
    let name = path.rsplit('/').next().unwrap_or(path);
    // Listed now, so under -delete a directory emptied of its entries counts
    // as empty.
    let is_empty_dir = if st.is_dir {
        io::list_dir(os, path)
            .await
            .map(|e| e.is_empty())
            .unwrap_or(false)
    } else {
        false
    };

    let mut delete_hit = false;
    let (matched, has_act) = eval_expr(expr, path, name, st, is_empty_dir, &mut delete_hit);
    if matched {
        if has_act {
            do_actions(
                os,
                proc,
                w,
                ew,
                expr,
                path,
                exec_plus_matches,
                exec_plus_cmd,
            )
            .await?;
        } else if default_print {
            w.write_all(path.as_bytes()).await?;
            w.write_all(b"\n").await?;
        }
    }

    // Like GNU find, -delete leaves the starting point "." in place and
    // refuses any other path that ends in "." or "..".
    let dot = matches!(
        path.trim_end_matches('/').rsplit('/').next(),
        Some("." | "..")
    );
    if delete_hit && path != "." {
        let removed = if dot {
            Err(std::io::Error::other("Invalid argument"))
        } else if st.is_dir {
            io::remove_dir(os, path).await
        } else {
            io::remove_file(os, path).await
        };
        if let Err(e) = removed {
            proc.err_msg(&format!("find: cannot delete '{path}': {e}"));
            *failed = true;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn do_actions<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    w: &'a mut crate::os::FdWriter,
    ew: &'a mut Option<crate::os::FdWriter>,
    expr: &'a Expr,
    path: &'a str,
    exec_plus_matches: &'a mut Vec<String>,
    exec_plus_cmd: &'a mut Option<Vec<String>>,
) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + 'a>> {
    Box::pin(async move {
        match expr {
            Expr::Print => {
                w.write_all(path.as_bytes()).await?;
                w.write_all(b"\n").await?;
            }
            Expr::Print0 => {
                w.write_all(path.as_bytes()).await?;
                w.write_all(b"\0").await?;
            }
            Expr::Exec(cmd_template, plus) => {
                if *plus {
                    *exec_plus_cmd = Some(cmd_template.clone());
                    exec_plus_matches.push(path.to_string());
                } else {
                    let parts: Vec<String> = cmd_template
                        .iter()
                        .map(|a| {
                            if a == "{}" {
                                path.to_string()
                            } else {
                                a.replace("{}", path)
                            }
                        })
                        .collect();
                    let line = parts
                        .iter()
                        .map(|s| shell_quote(s))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let os_arc = io::kernel();
                    let mut sub = exec_fork(proc);
                    let _ = Box::pin(crate::exec::execute(os_arc, &mut sub, &line)).await;
                    w.write_all(sub.captured_output.as_bytes()).await?;
                    if let Some(ew) = ew.as_mut() {
                        ew.write_all(sub.captured_stderr.as_bytes()).await?;
                    }
                }
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                do_actions(os, proc, w, ew, a, path, exec_plus_matches, exec_plus_cmd).await?;
                do_actions(os, proc, w, ew, b, path, exec_plus_matches, exec_plus_cmd).await?;
            }
            Expr::Not(inner) => {
                do_actions(
                    os,
                    proc,
                    w,
                    ew,
                    inner,
                    path,
                    exec_plus_matches,
                    exec_plus_cmd,
                )
                .await?;
            }
            _ => {}
        }
        Ok(())
    })
}
