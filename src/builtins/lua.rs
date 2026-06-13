use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use mlua::prelude::*;

use crate::commands::CommandResult;
use crate::exec;
use crate::io as sio;
use crate::os::{self, Kernel, OpenFlags, Process};

pub fn builtin_lua<'a>(
    os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut script_file: Option<String> = None;
        let mut eval_code: Option<String> = None;
        let mut interactive = false;
        let mut script_args: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-e" => {
                    i += 1;
                    if i >= args.len() {
                        proc.err_msg("lua: '-e' needs argument");
                        return Ok(1);
                    }
                    eval_code = Some(args[i].clone());
                }
                "-i" => interactive = true,
                s if s.starts_with('-') && s != "-" && s != "--" => {
                    proc.err_msg(&format!("lua: unrecognized option '{s}'"));
                    return Ok(1);
                }
                "--" => {
                    if i + 1 < args.len() {
                        script_file = Some(args[i + 1].clone());
                        script_args = args[i + 2..].to_vec();
                    }
                    break;
                }
                _ => {
                    script_file = Some(args[i].clone());
                    script_args = args[i + 1..].to_vec();
                    break;
                }
            }
            i += 1;
        }

        // Detect REPL mode: -i flag, or no script/eval and stdin unavailable
        let repl_mode = interactive
            || (script_file.is_none()
                && eval_code.is_none()
                && !sio::with_process(|p| p.has_fd(os::STDIN)));

        if repl_mode {
            return run_repl(os, proc).await;
        }

        // Read script source through the kernel
        let code = if let Some(ref code) = eval_code {
            code.clone()
        } else if let Some(ref path) = script_file {
            let fd = os.open(proc, path, OpenFlags::read()).await.map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("lua: {path}: {e}").into()
                },
            )?;
            let mut reader = proc.take_reader(fd)?;
            os::read_to_string_limited(&mut reader, proc.max_output)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("lua: {path}: {e}").into()
                })?
        } else {
            // Read from stdin
            let mut reader = sio::stdin()?;
            os::read_to_string_limited(&mut reader, proc.max_output)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("lua: {e}").into()
                })?
        };

        // Pre-read stdin for io.read() when script comes from file or -e
        let stdin_data = if script_file.is_some() || eval_code.is_some() {
            if let Ok(mut r) = sio::stdin() {
                os::read_to_string_limited(&mut r, proc.max_output)
                    .await
                    .unwrap_or_default()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Strip shebang
        let code = if code.starts_with("#!") {
            code.split_once('\n').map(|x| x.1).unwrap_or("").to_string()
        } else {
            code
        };

        let stdout_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let stderr_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));

        let lua = setup_lua_vm(proc, &script_args, &stdin_data, &stdout_buf, &stderr_buf)?;

        let chunk_name = script_file.as_deref().unwrap_or("=stdin");
        let result = match lua.load(&code).set_name(chunk_name).exec_async().await {
            Ok(()) => 0,
            Err(e) => {
                let msg = e.to_string();
                if let Some(pos) = msg.find("__strands_shell_exit:") {
                    let rest = &msg[pos + "__strands_shell_exit:".len()..];
                    let code_str: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '-')
                        .collect();
                    code_str.parse::<i32>().unwrap_or(1)
                } else {
                    stderr_buf
                        .borrow_mut()
                        .extend_from_slice(format!("{e}\n").as_bytes());
                    1
                }
            }
        };

        let _ = flush_lua_output(&stdout_buf, &stderr_buf).await;

        Ok(result)
    })
}

pub fn setup_lua_vm(
    proc: &mut Process,
    script_args: &[String],
    stdin_data: &str,
    stdout_buf: &Rc<RefCell<Vec<u8>>>,
    stderr_buf: &Rc<RefCell<Vec<u8>>>,
) -> Result<Lua, Box<dyn std::error::Error + Send + Sync>> {
    let safe_libs = LuaStdLib::STRING
        | LuaStdLib::TABLE
        | LuaStdLib::MATH
        | LuaStdLib::UTF8
        | LuaStdLib::COROUTINE;
    let lua = Lua::new_with(safe_libs, LuaOptions::default())
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("lua: {e}").into() })?;

    // Limit Lua memory to 100 MB to prevent exhaustion attacks
    // (e.g., string.rep("A", 1e9) allocates in one instruction
    // before the timeout hook fires)
    let _ = lua.set_memory_limit(100 * 1024 * 1024);

    if let Some(deadline) = proc.deadline {
        lua.set_app_data(deadline);
        lua.set_global_hook(
            mlua::HookTriggers::new().every_nth_instruction(4096),
            |lua, _debug| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let Some(dl) = lua.app_data_ref::<tokio::time::Instant>()
                        && tokio::time::Instant::now() >= *dl
                    {
                        return Err(LuaError::external("execution timeout exceeded"));
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some(dl) = lua.app_data_ref::<std::time::Instant>() {
                        if std::time::Instant::now() >= *dl {
                            return Err(LuaError::external("execution timeout exceeded"));
                        }
                    }
                }
                Ok(mlua::VmState::Continue)
            },
        )
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("lua: {e}").into() })?;
    }

    let kernel = sio::kernel();
    setup_sandbox(
        &lua,
        kernel,
        proc,
        script_args,
        stdin_data,
        stdout_buf,
        stderr_buf,
    )
    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("lua: {e}").into() })?;

    {
        let g = lua.globals();
        for name in [
            "load",
            "collectgarbage",
            "rawset",
            "rawget",
            "rawequal",
            "rawlen",
            "setmetatable",
            "getmetatable",
            "warn",
        ] {
            let _ = g.raw_set(name, mlua::Value::Nil);
        }
    }

    Ok(lua)
}

async fn flush_lua_output(
    stdout_buf: &Rc<RefCell<Vec<u8>>>,
    stderr_buf: &Rc<RefCell<Vec<u8>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::AsyncWriteExt;
    {
        let mut buf = stdout_buf.borrow_mut();
        if !buf.is_empty() {
            if let Ok(mut w) = sio::stdout() {
                w.write_all(&buf).await?;
            } else {
                // No VFS stdout (e.g. REPL mode) — write to real stdout
                std::io::Write::write_all(&mut std::io::stdout(), &buf)?;
            }
            buf.clear();
        }
    }
    {
        let mut buf = stderr_buf.borrow_mut();
        if !buf.is_empty() {
            if let Ok(mut w) = sio::stderr() {
                let _ = w.write_all(&buf).await;
            } else {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &buf);
            }
            buf.clear();
        }
    }
    Ok(())
}

/// Core REPL loop: reads lines via `read_line`, executes them in the Lua VM.
/// Output goes to `out` and `err` writers. Returns exit code.
pub async fn repl_loop(
    lua: &Lua,
    read_line: &mut dyn FnMut(&str) -> Option<String>,
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    stdout_buf: &Rc<RefCell<Vec<u8>>>,
    stderr_buf: &Rc<RefCell<Vec<u8>>>,
) -> i32 {
    let flush = |out: &mut dyn std::io::Write, err: &mut dyn std::io::Write| {
        let mut buf = stdout_buf.borrow_mut();
        if !buf.is_empty() {
            let _ = out.write_all(&buf);
            buf.clear();
        }
        let mut buf = stderr_buf.borrow_mut();
        if !buf.is_empty() {
            let _ = err.write_all(&buf);
            buf.clear();
        }
    };

    while let Some(first) = read_line("> ") {
        let mut code = first;

        loop {
            // Try as expression first (like standard Lua REPL: "return <expr>")
            let try_expr = format!("return {code}");
            if let Ok(vals) = lua
                .load(&try_expr)
                .set_name("=stdin")
                .eval_async::<LuaMultiValue>()
                .await
            {
                let parts: Vec<String> = vals
                    .iter()
                    .map(|v| val_to_string(lua, v).unwrap_or_else(|_| "?".into()))
                    .collect();
                if !(parts.is_empty() || parts.len() == 1 && parts[0] == "nil") {
                    let _ = writeln!(out, "{}", parts.join("\t"));
                }
                flush(out, err);
                break;
            }

            // Try as statement
            match lua.load(&code).set_name("=stdin").exec_async().await {
                Ok(()) => {
                    flush(out, err);
                    break;
                }
                Err(LuaError::SyntaxError {
                    incomplete_input: true,
                    ..
                }) => match read_line(">> ") {
                    Some(cont) => {
                        code.push('\n');
                        code.push_str(&cont);
                    }
                    None => break,
                },
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("__strands_shell_exit:") {
                        return 0;
                    }
                    let _ = writeln!(err, "{e}");
                    flush(out, err);
                    break;
                }
            }
        }
    }

    0
}

#[cfg(not(target_arch = "wasm32"))]
async fn run_repl(_os: &dyn Kernel, proc: &mut Process) -> CommandResult {
    let stdout_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let stderr_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));

    let lua = setup_lua_vm(proc, &[], "", &stdout_buf, &stderr_buf)?;

    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("lua: {e}").into() })?;

    let mut read_line = |prompt: &str| -> Option<String> {
        match rl.readline(prompt) {
            Ok(line) => {
                let _ = rl.add_history_entry(&line);
                Some(line)
            }
            Err(_) => None,
        }
    };

    let code = repl_loop(
        &lua,
        &mut read_line,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &stdout_buf,
        &stderr_buf,
    )
    .await;

    Ok(code)
}

#[cfg(target_arch = "wasm32")]
async fn run_repl(_os: &dyn Kernel, proc: &mut Process) -> CommandResult {
    let stdout_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let stderr_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));

    let lua = setup_lua_vm(proc, &[], "", &stdout_buf, &stderr_buf)?;

    // Simple line-based REPL without rustyline (no terminal features on WASM)
    let mut read_line = |prompt: &str| -> Option<String> {
        use std::io::Write;
        let _ = std::io::stdout().write_all(prompt.as_bytes());
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                }
                if line.ends_with('\r') {
                    line.pop();
                }
                Some(line)
            }
            Err(_) => None,
        }
    };

    let code = repl_loop(
        &lua,
        &mut read_line,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &stdout_buf,
        &stderr_buf,
    )
    .await;

    Ok(code)
}

fn lua_str(s: &LuaString) -> String {
    s.to_string_lossy().to_string()
}

fn strip_shebang(code: &str) -> &str {
    if code.starts_with("#!") {
        code.split_once('\n').map(|x| x.1).unwrap_or("")
    } else {
        code
    }
}

async fn read_vfs_file(
    kernel: &Arc<dyn Kernel>,
    cwd: &str,
    env: &Arc<std::collections::HashMap<String, String>>,
    path: &str,
    max_output: usize,
) -> LuaResult<String> {
    let mut tmp_proc = Process::new(cwd.into(), (**env).clone());
    let fd = kernel
        .open(&mut tmp_proc, path, OpenFlags::read())
        .await
        .map_err(|e| LuaError::external(format!("{path}: {e}")))?;
    let mut reader = tmp_proc
        .take_reader(fd)
        .map_err(|e| LuaError::external(format!("{path}: {e}")))?;
    os::read_to_string_limited(&mut reader, max_output)
        .await
        .map_err(|e| LuaError::external(format!("{path}: {e}")))
}

/// Create a cursor-backed file handle table for reading.
fn make_read_handle(lua: &Lua, data: Vec<u8>) -> LuaResult<LuaTable> {
    let cursor = Rc::new(RefCell::new(std::io::Cursor::new(data)));
    let ft = lua.create_table()?;

    let c = cursor.clone();
    ft.set(
        "read",
        lua.create_function(move |lua, (_self, fmt): (LuaValue, Option<LuaString>)| {
            read_cursor(lua, &c, fmt)
        })?,
    )?;

    let c = cursor.clone();
    ft.set(
        "lines",
        lua.create_function(move |lua, _self: LuaValue| {
            let c2 = c.clone();
            lua.create_function(move |lua, ()| read_cursor_line(lua, &c2))
        })?,
    )?;

    ft.set("close", lua.create_function(|_, _self: LuaValue| Ok(()))?)?;
    Ok(ft)
}

fn setup_sandbox(
    lua: &Lua,
    kernel: Arc<dyn Kernel>,
    proc: &mut Process,
    script_args: &[String],
    stdin_data: &str,
    stdout_buf: &Rc<RefCell<Vec<u8>>>,
    stderr_buf: &Rc<RefCell<Vec<u8>>>,
) -> LuaResult<()> {
    let globals = lua.globals();

    // `arg` table
    let arg_table = lua.create_table()?;
    for (i, a) in script_args.iter().enumerate() {
        arg_table.set(i as i64 + 1, a.as_str())?;
    }
    arg_table.set("n", script_args.len() as i64)?;
    globals.set("arg", arg_table)?;

    // -- print --
    let out = stdout_buf.clone();
    globals.set(
        "print",
        lua.create_function(move |lua, args: LuaMultiValue| {
            let mut s = String::new();
            for (i, val) in args.iter().enumerate() {
                if i > 0 {
                    s.push('\t');
                }
                s.push_str(&val_to_string(lua, val)?);
            }
            s.push('\n');
            out.borrow_mut().extend_from_slice(s.as_bytes());
            Ok(())
        })?,
    )?;

    // -- io module --
    let io_table = lua.create_table()?;

    // io.write
    let out = stdout_buf.clone();
    io_table.set(
        "write",
        lua.create_function(move |_, args: LuaMultiValue| {
            let mut buf = out.borrow_mut();
            for val in args.iter() {
                match val {
                    LuaValue::String(s) => buf.extend_from_slice(&s.as_bytes()),
                    LuaValue::Integer(n) => buf.extend_from_slice(format!("{n}").as_bytes()),
                    LuaValue::Number(n) => buf.extend_from_slice(format!("{n}").as_bytes()),
                    _ => return Err(LuaError::external("bad argument to 'write'")),
                }
            }
            Ok(())
        })?,
    )?;

    // io.read — reads from pre-buffered stdin
    let stdin_cursor = Rc::new(RefCell::new(std::io::Cursor::new(
        stdin_data.as_bytes().to_vec(),
    )));
    let r = stdin_cursor.clone();
    io_table.set(
        "read",
        lua.create_function(move |lua, fmt: Option<LuaString>| read_cursor(lua, &r, fmt))?,
    )?;

    // io.lines (stdin)
    let r = stdin_cursor.clone();
    io_table.set(
        "lines",
        lua.create_function(move |lua, path: Option<LuaString>| {
            if path.is_some() {
                return Err(LuaError::external(
                    "io.lines(filename) not supported; use io.open",
                ));
            }
            let r2 = r.clone();
            lua.create_function(move |lua, ()| read_cursor_line(lua, &r2))
        })?,
    )?;

    // io.open — async, reads/writes through kernel
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    let max_out = proc.max_output;
    io_table.set(
        "open",
        lua.create_async_function(move |lua, (path, mode): (LuaString, Option<LuaString>)| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            async move {
                let path_s = lua_str(&path);
                let mode_s = mode.as_ref().map(lua_str);
                let mode = mode_s.as_deref().unwrap_or("r");

                let mut tmp_proc = Process::new(cwd.into(), (*env).clone());

                if mode.starts_with('r') {
                    let fd = k
                        .open(&mut tmp_proc, &path_s, OpenFlags::read())
                        .await
                        .map_err(|e| LuaError::external(format!("{path_s}: {e}")))?;
                    let mut reader = tmp_proc
                        .take_reader(fd)
                        .map_err(|e| LuaError::external(format!("{path_s}: {e}")))?;
                    let content = os::read_to_string_limited(&mut reader, max_out)
                        .await
                        .map_err(|e| LuaError::external(format!("{path_s}: {e}")))?;
                    make_read_handle(&lua, content.into_bytes())
                } else {
                    // Write mode: buffer content, flush on close via kernel
                    let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
                    let ft = lua.create_table()?;

                    let b = buf.clone();
                    ft.set(
                        "write",
                        lua.create_function(move |_, (_self, args): (LuaValue, LuaMultiValue)| {
                            let mut w = b.borrow_mut();
                            for val in args.iter() {
                                match val {
                                    LuaValue::String(s) => w.extend_from_slice(&s.as_bytes()),
                                    LuaValue::Integer(n) => {
                                        w.extend_from_slice(format!("{n}").as_bytes())
                                    }
                                    LuaValue::Number(n) => {
                                        w.extend_from_slice(format!("{n}").as_bytes())
                                    }
                                    _ => return Err(LuaError::external("bad argument to 'write'")),
                                }
                            }
                            Ok(())
                        })?,
                    )?;

                    let b = buf.clone();
                    let k2 = k.clone();
                    let path_s2 = path_s.clone();
                    let cwd2 = tmp_proc.cwd.to_string_lossy().to_string();
                    let env2 = tmp_proc.env.clone();
                    ft.set(
                        "close",
                        lua.create_async_function(move |_, _self: LuaValue| {
                            let b = b.clone();
                            let k2 = k2.clone();
                            let path_s2 = path_s2.clone();
                            let cwd2 = cwd2.clone();
                            let env2 = env2.clone();
                            async move {
                                let data = b.borrow().clone();
                                let mut wp = Process::new(cwd2.into(), (*env2).clone());
                                let fd = k2
                                    .open(&mut wp, &path_s2, OpenFlags::write())
                                    .await
                                    .map_err(|e| LuaError::external(format!("{path_s2}: {e}")))?;
                                let mut writer = wp
                                    .take_writer(fd)
                                    .map_err(|e| LuaError::external(format!("{path_s2}: {e}")))?;
                                use tokio::io::AsyncWriteExt;
                                writer
                                    .write_all(&data)
                                    .await
                                    .map_err(|e| LuaError::external(format!("{path_s2}: {e}")))?;
                                drop(writer);
                                // Yield to let the VFS flush task drain the channel
                                tokio::task::yield_now().await;
                                Ok(())
                            }
                        })?,
                    )?;

                    Ok(ft)
                }
            }
        })?,
    )?;

    // io.popen — run shell command, return handle over captured output
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    let max_output = proc.max_output;
    let popen_deadline = proc.deadline;
    let popen_max_depth = proc.max_depth;
    let popen_depth = proc.depth;
    let popen_max_fds = proc.max_fds;
    let popen_max_bg = proc.max_bg_jobs;
    let popen_max_pipe = proc.max_pipeline;
    let popen_max_input = proc.max_input;
    io_table.set(
        "popen",
        lua.create_async_function(move |lua, (cmd, mode): (LuaString, Option<LuaString>)| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            async move {
                let cmd_s = lua_str(&cmd);
                let mode_s = mode.as_ref().map(lua_str);
                let mode = mode_s.as_deref().unwrap_or("r");

                if !mode.starts_with('r') {
                    return Err(LuaError::external("io.popen: only read mode supported"));
                }

                let mut sub_proc = Process::new(cwd.into(), (*env).clone());
                sub_proc.max_output = max_output;
                sub_proc.deadline = popen_deadline;
                sub_proc.max_depth = popen_max_depth;
                sub_proc.depth = popen_depth;
                sub_proc.max_fds = popen_max_fds;
                sub_proc.max_bg_jobs = popen_max_bg;
                sub_proc.max_pipeline = popen_max_pipe;
                sub_proc.max_input = popen_max_input;
                let (_exit, stdout, _stderr) =
                    exec::execute_capture(k, &mut sub_proc, &cmd_s).await;

                make_read_handle(&lua, stdout.into_bytes())
            }
        })?,
    )?;

    io_table.set("close", lua.create_function(|_, _: LuaValue| Ok(()))?)?;

    // io.stderr — write to stderr buffer
    let stderr_handle = lua.create_table()?;
    let err = stderr_buf.clone();
    stderr_handle.set(
        "write",
        lua.create_function(move |_, (_self, args): (LuaValue, LuaMultiValue)| {
            let mut buf = err.borrow_mut();
            for val in args.iter() {
                match val {
                    LuaValue::String(s) => buf.extend_from_slice(&s.as_bytes()),
                    LuaValue::Integer(n) => buf.extend_from_slice(format!("{n}").as_bytes()),
                    LuaValue::Number(n) => buf.extend_from_slice(format!("{n}").as_bytes()),
                    _ => return Err(LuaError::external("bad argument to 'write'")),
                }
            }
            Ok(())
        })?,
    )?;
    io_table.set("stderr", stderr_handle)?;

    globals.set("io", io_table)?;

    // -- os module (safe subset) --
    let os_table = lua.create_table()?;
    os_table.set(
        "clock",
        lua.create_function(|_, ()| {
            Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64())
        })?,
    )?;
    os_table.set(
        "time",
        lua.create_function(|_, ()| {
            Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64)
        })?,
    )?;
    os_table.set(
        "difftime",
        lua.create_function(|_, (t2, t1): (i64, i64)| Ok(t2 - t1))?,
    )?;

    // os.getenv — read from process env
    let env = proc.env.clone();
    os_table.set(
        "getenv",
        lua.create_function(move |lua, name: LuaString| {
            let name = lua_str(&name);
            match env.get(&name) {
                Some(v) => Ok(LuaValue::String(lua.create_string(v.as_bytes())?)),
                None => Ok(LuaNil),
            }
        })?,
    )?;

    // os.remove — async, through kernel
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    os_table.set(
        "remove",
        lua.create_async_function(move |_, path: LuaString| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            async move {
                let s = lua_str(&path);
                let p = Process::new(cwd.into(), (*env).clone());
                k.remove_file(&p, &s)
                    .await
                    .map_err(|e| LuaError::external(format!("{s}: {e}")))?;
                Ok(true)
            }
        })?,
    )?;

    // os.rename — async, through kernel
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    os_table.set(
        "rename",
        lua.create_async_function(move |_, (from, to): (LuaString, LuaString)| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            async move {
                let f = lua_str(&from);
                let t = lua_str(&to);
                let p = Process::new(cwd.into(), (*env).clone());
                k.rename(&p, &f, &t)
                    .await
                    .map_err(|e| LuaError::external(format!("{f}: {e}")))?;
                Ok(true)
            }
        })?,
    )?;

    // os.execute — run shell command through execute_capture
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    let max_output = proc.max_output;
    let exec_deadline = proc.deadline;
    let exec_max_depth = proc.max_depth;
    let exec_depth = proc.depth;
    let exec_max_fds = proc.max_fds;
    let exec_max_bg = proc.max_bg_jobs;
    let exec_max_pipe = proc.max_pipeline;
    let exec_max_input = proc.max_input;
    let out = stdout_buf.clone();
    let err = stderr_buf.clone();
    os_table.set(
        "execute",
        lua.create_async_function(move |lua, cmd: Option<LuaString>| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            let out = out.clone();
            let err = err.clone();
            async move {
                let cmd_s = match cmd {
                    Some(s) => lua_str(&s),
                    None => {
                        let exit_str = lua.create_string("exit")?;
                        return Ok((
                            LuaValue::Boolean(true),
                            LuaValue::String(exit_str),
                            LuaValue::Integer(0),
                        ));
                    }
                };
                let mut sub_proc = Process::new(cwd.into(), (*env).clone());
                sub_proc.max_output = max_output;
                sub_proc.deadline = exec_deadline;
                sub_proc.max_depth = exec_max_depth;
                sub_proc.depth = exec_depth;
                sub_proc.max_fds = exec_max_fds;
                sub_proc.max_bg_jobs = exec_max_bg;
                sub_proc.max_pipeline = exec_max_pipe;
                sub_proc.max_input = exec_max_input;
                let (code, stdout, stderr) = exec::execute_capture(k, &mut sub_proc, &cmd_s).await;
                if !stdout.is_empty() {
                    out.borrow_mut().extend_from_slice(stdout.as_bytes());
                }
                if !stderr.is_empty() {
                    err.borrow_mut().extend_from_slice(stderr.as_bytes());
                }
                let exit_str = lua.create_string("exit")?;
                if code == 0 {
                    Ok((
                        LuaValue::Boolean(true),
                        LuaValue::String(exit_str),
                        LuaValue::Integer(0),
                    ))
                } else {
                    Ok((
                        LuaNil,
                        LuaValue::String(exit_str),
                        LuaValue::Integer(code as i64),
                    ))
                }
            }
        })?,
    )?;
    // os.exit — signal early termination via custom error
    os_table.set(
        "exit",
        lua.create_function(|_, code: Option<LuaValue>| -> LuaResult<()> {
            let code = match code {
                None | Some(LuaValue::Boolean(true)) => 0i64,
                Some(LuaValue::Boolean(false)) => 1,
                Some(LuaValue::Integer(n)) => n,
                Some(LuaValue::Number(n)) => n as i64,
                _ => 0,
            };
            Err(LuaError::external(format!("__strands_shell_exit:{code}")))
        })?,
    )?;
    globals.set("os", os_table)?;

    // -- dofile: load and execute a Lua file through the VFS --
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    let max_out = proc.max_output;
    globals.set(
        "dofile",
        lua.create_async_function(move |lua, path: Option<LuaString>| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            async move {
                let path_s = match path {
                    Some(p) => lua_str(&p),
                    None => return Err(LuaError::external("dofile: filename required")),
                };
                let code = read_vfs_file(&k, &cwd, &env, &path_s, max_out).await?;
                let code = strip_shebang(&code);
                lua.load(code)
                    .set_name(&path_s)
                    .eval_async::<LuaMultiValue>()
                    .await
            }
        })?,
    )?;

    // -- loadfile: compile a Lua file to a function without executing --
    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    let max_out = proc.max_output;
    globals.set("loadfile", lua.create_async_function(move |lua, (path, _mode, _env): (Option<LuaString>, Option<LuaString>, Option<LuaValue>)| {
        let k = k.clone();
        let cwd = cwd.clone();
        let env = env.clone();
        async move {
            let path_s = match path {
                Some(p) => lua_str(&p),
                None => return Err(LuaError::external("loadfile: filename required")),
            };
            let code = read_vfs_file(&k, &cwd, &env, &path_s, max_out).await?;
            let code = strip_shebang(&code);
            let func = lua.load(code).set_name(&path_s).into_function()
                .map_err(|e| LuaError::external(format!("{e}")))?;
            Ok(func)
        }
    })?)?;

    // -- require: search for module, load, cache in package.loaded --
    let loaded = lua.create_table()?;
    let pkg = lua.create_table()?;
    pkg.set("loaded", loaded.clone())?;
    pkg.set(
        "path",
        "/usr/share/lua/?.lua;/usr/share/lua/?/init.lua;./?.lua;./?/init.lua",
    )?;
    globals.set("package", pkg)?;

    let k = kernel.clone();
    let cwd = proc.cwd.to_string_lossy().to_string();
    let env = proc.env.clone();
    let max_out = proc.max_output;
    globals.set(
        "require",
        lua.create_async_function(move |lua, modname: LuaString| {
            let k = k.clone();
            let cwd = cwd.clone();
            let env = env.clone();
            async move {
                let name = lua_str(&modname);

                // Check cache
                let pkg: LuaTable = lua.globals().get("package")?;
                let loaded: LuaTable = pkg.get("loaded")?;
                if let Ok(val) = loaded.get::<LuaValue>(name.as_str())
                    && val != LuaNil
                {
                    // Yield to runtime before returning cached value
                    // (fixes mlua async function early-return issue in Python bindings)
                    tokio::task::yield_now().await;
                    return Ok(val);
                }

                // Search package.path
                let search_path: String = pkg.get("path")?;
                let mut last_err = String::new();
                for template in search_path.split(';') {
                    let path = template.replace('?', &name.replace('.', "/"));
                    match read_vfs_file(&k, &cwd, &env, &path, max_out).await {
                        Ok(code) => {
                            let code = strip_shebang(&code);
                            let result = lua
                                .load(code)
                                .set_name(&path)
                                .eval_async::<LuaValue>()
                                .await
                                .map_err(|e| LuaError::external(format!("{e}")))?;
                            let val = if result == LuaNil {
                                LuaValue::Boolean(true)
                            } else {
                                result.clone()
                            };
                            loaded.set(name.as_str(), val)?;
                            return Ok(result);
                        }
                        Err(_) => {
                            last_err.push_str(&format!("\n\tno file '{path}'"));
                        }
                    }
                }
                Err(LuaError::external(format!(
                    "module '{name}' not found:{last_err}"
                )))
            }
        })?,
    )?;

    // Register MCP tool modules into package.loaded (not available on WASM)
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(clients) = sio::mcp_clients() {
        for client in clients.iter() {
            let mod_table = lua.create_table()?;
            for tool in &client.client.tools {
                let tool_name = tool.name.clone();
                let clients_ref = clients.clone();
                let mod_idx = clients
                    .iter()
                    .position(|c| std::ptr::eq(c, client))
                    .unwrap();
                mod_table.set(
                    tool.name.as_str(),
                    lua.create_async_function(move |lua, args: Option<LuaTable>| {
                        let tool_name = tool_name.clone();
                        let clients_ref = clients_ref.clone();
                        async move {
                            let json_args = match args {
                                Some(t) => lua_table_to_json(&lua, &t)?,
                                None => serde_json::Value::Object(serde_json::Map::new()),
                            };
                            let result = clients_ref[mod_idx]
                                .client
                                .call_tool(&tool_name, json_args)
                                .await
                                .map_err(|e| LuaError::external(e.to_string()))?;
                            // Extract text content from MCP response
                            mcp_result_to_lua(&lua, &result)
                        }
                    })?,
                )?;
            }
            loaded.set(client.module_name.as_str(), mod_table)?;
        }
    }

    Ok(())
}

fn read_cursor_line(lua: &Lua, c: &Rc<RefCell<std::io::Cursor<Vec<u8>>>>) -> LuaResult<LuaValue> {
    use std::io::BufRead;
    let mut cur = c.borrow_mut();
    let mut line = String::new();
    let n = cur.read_line(&mut line).map_err(LuaError::external)?;
    if n == 0 {
        return Ok(LuaNil);
    }
    if line.ends_with('\n') {
        line.pop();
    }
    Ok(LuaValue::String(lua.create_string(&line)?))
}

fn read_cursor(
    lua: &Lua,
    c: &Rc<RefCell<std::io::Cursor<Vec<u8>>>>,
    fmt: Option<LuaString>,
) -> LuaResult<LuaValue> {
    use std::io::BufRead;
    let f = fmt.as_ref().map(lua_str);
    let f = f.as_deref().unwrap_or("*l");
    let mut cur = c.borrow_mut();
    match f {
        "*l" | "l" => {
            let mut line = String::new();
            let n = cur.read_line(&mut line).map_err(LuaError::external)?;
            if n == 0 {
                return Ok(LuaNil);
            }
            if line.ends_with('\n') {
                line.pop();
            }
            Ok(LuaValue::String(lua.create_string(&line)?))
        }
        "*a" | "a" => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut *cur, &mut buf).map_err(LuaError::external)?;
            Ok(LuaValue::String(lua.create_string(&buf)?))
        }
        "*n" | "n" => {
            let mut line = String::new();
            let n = cur.read_line(&mut line).map_err(LuaError::external)?;
            if n == 0 {
                return Ok(LuaNil);
            }
            match line.trim().parse::<f64>() {
                Ok(n) => Ok(LuaValue::Number(n)),
                Err(_) => Ok(LuaNil),
            }
        }
        _ => Err(LuaError::external(format!("unsupported format '{f}'"))),
    }
}

fn val_to_string(lua: &Lua, val: &LuaValue) -> LuaResult<String> {
    match val {
        LuaValue::Nil => Ok("nil".into()),
        LuaValue::Boolean(b) => Ok(b.to_string()),
        LuaValue::Integer(n) => Ok(n.to_string()),
        LuaValue::Number(n) => Ok(format!("{n}")),
        LuaValue::String(s) => Ok(s.to_string_lossy().to_string()),
        _ => {
            if let Ok(ts) = lua.globals().get::<LuaFunction>("tostring")
                && let Ok(s) = ts.call::<LuaString>(val.clone())
            {
                return Ok(s.to_string_lossy().to_string());
            }
            Ok(format!("{val:?}"))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn lua_table_to_json(lua: &Lua, table: &LuaTable) -> LuaResult<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for pair in table.pairs::<LuaString, LuaValue>() {
        let (key, val) = pair?;
        map.insert(lua_str(&key), lua_value_to_json(lua, &val)?);
    }
    Ok(serde_json::Value::Object(map))
}

#[cfg(not(target_arch = "wasm32"))]
fn lua_value_to_json(lua: &Lua, val: &LuaValue) -> LuaResult<serde_json::Value> {
    match val {
        LuaValue::Nil => Ok(serde_json::Value::Null),
        LuaValue::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        LuaValue::Integer(n) => Ok(serde_json::json!(*n)),
        LuaValue::Number(n) => Ok(serde_json::json!(*n)),
        LuaValue::String(s) => Ok(serde_json::Value::String(lua_str(s))),
        LuaValue::Table(t) => {
            // Check if it's an array (sequential integer keys starting at 1)
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::new();
                for i in 1..=len {
                    let v: LuaValue = t.get(i)?;
                    arr.push(lua_value_to_json(lua, &v)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                lua_table_to_json(lua, t)
            }
        }
        _ => Ok(serde_json::Value::Null),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn json_to_lua_value(lua: &Lua, val: &serde_json::Value) -> LuaResult<LuaValue> {
    match val {
        serde_json::Value::Null => Ok(LuaNil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(s.as_bytes())?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, item) in arr.iter().enumerate() {
                t.set(i + 1, json_to_lua_value(lua, item)?)?;
            }
            Ok(LuaValue::Table(t))
        }
        serde_json::Value::Object(obj) => {
            let t = lua.create_table()?;
            for (k, v) in obj {
                t.set(k.as_str(), json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(t))
        }
    }
}

/// Convert an MCP tools/call result to a Lua value.
/// If the result contains text content, returns the text as a string.
/// If it contains structured content, converts to a Lua table.
#[cfg(not(target_arch = "wasm32"))]
fn mcp_result_to_lua(lua: &Lua, result: &serde_json::Value) -> LuaResult<LuaValue> {
    // Check for isError
    if result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("MCP tool error");
        return Err(LuaError::external(msg));
    }

    // Extract content array
    let content = match result.get("content").and_then(|c| c.as_array()) {
        Some(arr) => arr,
        None => return json_to_lua_value(lua, result),
    };

    // Single text content → return as string
    if content.len() == 1
        && let Some(text) = content[0].get("text").and_then(|t| t.as_str())
    {
        return Ok(LuaValue::String(lua.create_string(text.as_bytes())?));
    }

    // Multiple content items → return as table
    let t = lua.create_table()?;
    for (i, item) in content.iter().enumerate() {
        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
            t.set(i + 1, lua.create_string(text.as_bytes())?)?;
        }
    }
    Ok(LuaValue::Table(t))
}
