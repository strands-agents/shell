use strands_shell::Shell;

fn rt() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    (rt, local)
}

macro_rules! lua_expect {
    ($name:ident, $cmd:expr, $stdout:expr) => {
        #[test]
        fn $name() {
            let (rt, local) = rt();
            rt.block_on(local.run_until(async {
                let mut shell = Shell::builder().build().unwrap();
                let out = shell.run($cmd).await;
                assert_eq!(
                    out.stdout.trim(),
                    $stdout,
                    "stdout mismatch\nstderr: {}",
                    out.stderr
                );
                assert_eq!(
                    out.status, 0,
                    "expected exit 0, got {}\nstderr: {}",
                    out.status, out.stderr
                );
            }));
        }
    };
}

macro_rules! lua_status {
    ($name:ident, $cmd:expr, $status:expr) => {
        #[test]
        fn $name() {
            let (rt, local) = rt();
            rt.block_on(local.run_until(async {
                let mut shell = Shell::builder().build().unwrap();
                let out = shell.run($cmd).await;
                assert_eq!(
                    out.status, $status,
                    "exit status mismatch\nstdout: {}\nstderr: {}",
                    out.stdout, out.stderr
                );
            }));
        }
    };
}

macro_rules! lua_stderr {
    ($name:ident, $cmd:expr, $pat:expr) => {
        #[test]
        fn $name() {
            let (rt, local) = rt();
            rt.block_on(local.run_until(async {
                let mut shell = Shell::builder().build().unwrap();
                let out = shell.run($cmd).await;
                assert!(
                    out.stderr.contains($pat),
                    "stderr should contain {:?}, got: {}",
                    $pat,
                    out.stderr
                );
            }));
        }
    };
}

// ── print / basic execution ─────────────────────────────────────────

lua_expect!(lua_print_hello, "lua -e 'print(\"hello\")'", "hello");
lua_expect!(lua_print_number, "lua -e 'print(42)'", "42");
lua_expect!(lua_print_bool, "lua -e 'print(true)'", "true");
lua_expect!(lua_print_nil, "lua -e 'print(nil)'", "nil");
lua_expect!(lua_print_multi, "lua -e 'print(1, 2, 3)'", "1\t2\t3");
lua_expect!(lua_print_concat, "lua -e 'print(\"a\" .. \"b\")'", "ab");

// ── arithmetic ──────────────────────────────────────────────────────

lua_expect!(lua_arith_add, "lua -e 'print(1 + 2)'", "3");
lua_expect!(lua_arith_mul, "lua -e 'print(3 * 4)'", "12");
lua_expect!(
    lua_arith_div,
    "lua -e 'print(10 / 3)'",
    "3.3333333333333335"
);
lua_expect!(lua_arith_idiv, "lua -e 'print(10 // 3)'", "3");
lua_expect!(lua_arith_mod, "lua -e 'print(10 % 3)'", "1");
lua_expect!(lua_arith_pow, "lua -e 'print(2 ^ 10)'", "1024");
lua_expect!(lua_arith_neg, "lua -e 'print(-42)'", "-42");

// ── string library ──────────────────────────────────────────────────

lua_expect!(lua_string_len, "lua -e 'print(string.len(\"hello\"))'", "5");
lua_expect!(
    lua_string_upper,
    "lua -e 'print(string.upper(\"hello\"))'",
    "HELLO"
);
lua_expect!(
    lua_string_lower,
    "lua -e 'print(string.lower(\"HELLO\"))'",
    "hello"
);
lua_expect!(
    lua_string_rep,
    "lua -e 'print(string.rep(\"ab\", 3))'",
    "ababab"
);
lua_expect!(
    lua_string_reverse,
    "lua -e 'print(string.reverse(\"hello\"))'",
    "olleh"
);
lua_expect!(
    lua_string_sub,
    "lua -e 'print(string.sub(\"hello\", 2, 4))'",
    "ell"
);
lua_expect!(
    lua_string_find,
    "lua -e 'print(string.find(\"hello world\", \"world\"))'",
    "7\t11"
);
lua_expect!(
    lua_string_format,
    "lua -e 'print(string.format(\"%d %s\", 42, \"hi\"))'",
    "42 hi"
);
lua_expect!(lua_string_byte, "lua -e 'print(string.byte(\"A\"))'", "65");
lua_expect!(
    lua_string_char,
    "lua -e 'print(string.char(65, 66, 67))'",
    "ABC"
);
lua_expect!(
    lua_string_gsub,
    "lua -e 'print(string.gsub(\"hello\", \"l\", \"L\"))'",
    "heLLo\t2"
);
lua_expect!(
    lua_string_match,
    "lua -e 'print(string.match(\"hello123\", \"%d+\"))'",
    "123"
);
lua_expect!(
    lua_string_gmatch,
    "lua -e 'local t={} for w in string.gmatch(\"a b c\", \"%S+\") do t[#t+1]=w end print(table.concat(t,\",\"))'",
    "a,b,c"
);

// ── table library ───────────────────────────────────────────────────

lua_expect!(
    lua_table_concat,
    "lua -e 'print(table.concat({\"a\",\"b\",\"c\"}, \",\"))'",
    "a,b,c"
);
lua_expect!(
    lua_table_insert,
    "lua -e 'local t={1,2} table.insert(t,3) print(t[3])'",
    "3"
);
lua_expect!(
    lua_table_remove,
    "lua -e 'local t={1,2,3} table.remove(t,2) print(t[1],t[2])'",
    "1\t3"
);
lua_expect!(
    lua_table_sort,
    "lua -e 'local t={3,1,2} table.sort(t) print(t[1],t[2],t[3])'",
    "1\t2\t3"
);
lua_expect!(
    lua_table_sort_custom,
    "lua -e 'local t={1,2,3} table.sort(t, function(a,b) return a>b end) print(t[1],t[2],t[3])'",
    "3\t2\t1"
);
lua_expect!(
    lua_table_move,
    "lua -e 'local t={1,2,3,4,5} table.move(t,3,5,1) print(t[1],t[2],t[3])'",
    "3\t4\t5"
);
lua_expect!(
    lua_table_unpack,
    "lua -e 'print(table.unpack({10,20,30}))'",
    "10\t20\t30"
);
lua_expect!(
    lua_table_pack,
    "lua -e 'local t=table.pack(1,2,3) print(t.n, t[1], t[2], t[3])'",
    "3\t1\t2\t3"
);

// ── math library ────────────────────────────────────────────────────

lua_expect!(lua_math_abs, "lua -e 'print(math.abs(-5))'", "5");
lua_expect!(lua_math_floor, "lua -e 'print(math.floor(3.7))'", "3");
lua_expect!(lua_math_ceil, "lua -e 'print(math.ceil(3.2))'", "4");
lua_expect!(lua_math_max, "lua -e 'print(math.max(1,5,3))'", "5");
lua_expect!(lua_math_min, "lua -e 'print(math.min(1,5,3))'", "1");
lua_expect!(lua_math_sqrt, "lua -e 'print(math.sqrt(16))'", "4");
lua_expect!(
    lua_math_pi,
    "lua -e 'print(math.pi > 3.14 and math.pi < 3.15)'",
    "true"
);
lua_expect!(lua_math_huge, "lua -e 'print(math.huge > 0)'", "true");
lua_expect!(lua_math_type_int, "lua -e 'print(math.type(1))'", "integer");
lua_expect!(
    lua_math_type_float,
    "lua -e 'print(math.type(1.0))'",
    "float"
);
lua_expect!(
    lua_math_tointeger,
    "lua -e 'print(math.tointeger(5.0))'",
    "5"
);

// ── control flow ────────────────────────────────────────────────────

lua_expect!(
    lua_if_true,
    "lua -e 'if true then print(\"yes\") end'",
    "yes"
);
lua_expect!(
    lua_if_else,
    "lua -e 'if false then print(\"no\") else print(\"yes\") end'",
    "yes"
);
lua_expect!(
    lua_if_elseif,
    "lua -e 'local x=2 if x==1 then print(\"a\") elseif x==2 then print(\"b\") else print(\"c\") end'",
    "b"
);
lua_expect!(
    lua_for_numeric,
    "lua -e 'local s=0 for i=1,10 do s=s+i end print(s)'",
    "55"
);
lua_expect!(
    lua_for_step,
    "lua -e 'local s=0 for i=1,10,2 do s=s+i end print(s)'",
    "25"
);
lua_expect!(
    lua_for_in_ipairs,
    "lua -e 'local s=0 for _,v in ipairs({10,20,30}) do s=s+v end print(s)'",
    "60"
);
lua_expect!(
    lua_for_in_pairs,
    "lua -e 'local t={a=1} for k,v in pairs(t) do print(k,v) end'",
    "a\t1"
);
lua_expect!(
    lua_while,
    "lua -e 'local i=0 while i<5 do i=i+1 end print(i)'",
    "5"
);
lua_expect!(
    lua_repeat,
    "lua -e 'local i=0 repeat i=i+1 until i>=5 print(i)'",
    "5"
);

// ── functions ───────────────────────────────────────────────────────

lua_expect!(
    lua_func_basic,
    "lua -e 'local function f(x) return x*2 end print(f(21))'",
    "42"
);
lua_expect!(
    lua_func_multi_return,
    "lua -e 'local function f() return 1,2,3 end print(f())'",
    "1\t2\t3"
);
lua_expect!(
    lua_func_varargs,
    "lua -e 'local function f(...) return select(\"#\", ...) end print(f(1,2,3))'",
    "3"
);
lua_expect!(
    lua_func_closure,
    "lua -e 'local function make(x) return function() return x end end print(make(42)())'",
    "42"
);
lua_expect!(
    lua_func_recursive,
    "lua -e 'local function fib(n) if n<2 then return n end return fib(n-1)+fib(n-2) end print(fib(10))'",
    "55"
);

// ── type / tostring / tonumber / select / pcall / error ─────────────

lua_expect!(lua_type_string, "lua -e 'print(type(\"hi\"))'", "string");
lua_expect!(lua_type_number, "lua -e 'print(type(42))'", "number");
lua_expect!(lua_type_table, "lua -e 'print(type({}))'", "table");
lua_expect!(lua_type_bool, "lua -e 'print(type(true))'", "boolean");
lua_expect!(lua_type_nil, "lua -e 'print(type(nil))'", "nil");
lua_expect!(lua_type_func, "lua -e 'print(type(print))'", "function");
lua_expect!(lua_tostring, "lua -e 'print(tostring(42))'", "42");
lua_expect!(lua_tonumber, "lua -e 'print(tonumber(\"42\"))'", "42");
lua_expect!(
    lua_tonumber_base,
    "lua -e 'print(tonumber(\"ff\", 16))'",
    "255"
);
lua_expect!(
    lua_select_idx,
    "lua -e 'print(select(2, \"a\", \"b\", \"c\"))'",
    "b\tc"
);
lua_expect!(
    lua_select_count,
    "lua -e 'print(select(\"#\", \"a\", \"b\", \"c\"))'",
    "3"
);
lua_expect!(
    lua_pcall_ok,
    "lua -e 'local ok,v = pcall(function() return 42 end) print(ok,v)'",
    "true\t42"
);
lua_expect!(
    lua_pcall_err,
    "lua -e 'local ok,e = pcall(function() error(\"boom\") end) print(ok, type(e))'",
    "false\tstring"
);
lua_expect!(
    lua_xpcall,
    "lua -e 'local ok,e = xpcall(function() error(\"x\") end, function(e) return \"caught:\"..e end) print(ok,e)'",
    "false\tcaught:stdin:1: x"
);
lua_expect!(
    lua_error_string,
    "lua -e 'local ok,e = pcall(error, \"msg\") print(e)'",
    "msg"
);
lua_expect!(lua_assert_ok, "lua -e 'print(assert(42))'", "42");

// ── io.write ────────────────────────────────────────────────────────

lua_expect!(lua_io_write_string, "lua -e 'io.write(\"hello\")'", "hello");
lua_expect!(lua_io_write_number, "lua -e 'io.write(42)'", "42");
lua_expect!(
    lua_io_write_multi,
    "lua -e 'io.write(\"a\", \"b\", \"c\")'",
    "abc"
);
lua_expect!(lua_io_write_float, "lua -e 'io.write(3.14)'", "3.14");

// ── io.read (from stdin piped) ──────────────────────────────────────

lua_expect!(
    lua_io_read_all,
    "echo 'hello world' | lua -e 'print(io.read(\"*a\"))'",
    "hello world"
);
lua_expect!(
    lua_io_read_line,
    "printf 'line1\\nline2\\n' | lua -e 'print(io.read(\"*l\"))'",
    "line1"
);
lua_expect!(
    lua_io_read_number,
    "echo '42' | lua -e 'print(io.read(\"*n\"))'",
    "42"
);
lua_expect!(
    lua_io_read_default_line,
    "printf 'abc\\ndef\\n' | lua -e 'print(io.read())'",
    "abc"
);

// ── io.lines (stdin) ────────────────────────────────────────────────

lua_expect!(
    lua_io_lines_stdin,
    "printf 'a\\nb\\nc\\n' | lua -e 'local t={} for l in io.lines() do t[#t+1]=l end print(table.concat(t,\",\"))'",
    "a,b,c"
);

// ── io.open (read) ──────────────────────────────────────────────────

lua_expect!(
    lua_io_open_read,
    "echo hello > /tmp/t.txt && lua -e 'local f=io.open(\"/tmp/t.txt\",\"r\") print(f:read(\"*a\")) f:close()'",
    "hello"
);
lua_expect!(
    lua_io_open_read_line,
    "printf 'x\\ny\\n' > /tmp/t2.txt && lua -e 'local f=io.open(\"/tmp/t2.txt\") print(f:read(\"*l\")) f:close()'",
    "x"
);
lua_expect!(
    lua_io_open_lines,
    "printf 'p\\nq\\n' > /tmp/tl.txt && lua -e 'local t={} local f=io.open(\"/tmp/tl.txt\") for l in f:lines() do t[#t+1]=l end print(table.concat(t,\",\"))'",
    "p,q"
);

// ── io.open (write) ─────────────────────────────────────────────────

lua_expect!(
    lua_io_open_write,
    "lua -e 'local f=io.open(\"/tmp/w.txt\",\"w\") f:write(\"data\") f:close()' && cat /tmp/w.txt",
    "data"
);
lua_expect!(
    lua_io_open_write_multi,
    "lua -e 'local f=io.open(\"/tmp/wm.txt\",\"w\") f:write(\"a\",\"b\") f:close()' && cat /tmp/wm.txt",
    "ab"
);

// ── io.popen ────────────────────────────────────────────────────────

lua_expect!(
    lua_io_popen_read,
    "lua -e 'local f=io.popen(\"echo hi\") print(f:read(\"*a\"))'",
    "hi"
);
lua_expect!(
    lua_io_popen_lines,
    "printf 'a\\nb\\n' > /tmp/pl.txt && lua -e 'local f=io.popen(\"cat /tmp/pl.txt\") local t={} for l in f:lines() do t[#t+1]=l end print(table.concat(t,\",\"))'",
    "a,b"
);

// ── io.stderr ───────────────────────────────────────────────────────

#[test]
fn lua_io_stderr_write() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("lua -e 'io.stderr:write(\"err msg\")'").await;
        assert_eq!(out.status, 0);
        assert!(out.stderr.contains("err msg"), "stderr: {}", out.stderr);
    }));
}

// ── os module ───────────────────────────────────────────────────────

lua_expect!(lua_os_clock, "lua -e 'print(type(os.clock()))'", "number");
lua_expect!(lua_os_time, "lua -e 'print(os.time() > 0)'", "true");
lua_expect!(lua_os_difftime, "lua -e 'print(os.difftime(10, 3))'", "7");

// os.getenv
lua_expect!(
    lua_os_getenv_path,
    "lua -e 'print(os.getenv(\"HOME\"))'",
    "/home/lash"
);
lua_expect!(
    lua_os_getenv_nil,
    "lua -e 'print(os.getenv(\"NONEXISTENT_VAR_XYZ\"))'",
    "nil"
);

// os.execute
lua_expect!(
    lua_os_execute_true,
    "lua -e 'local ok,_,code = os.execute(\"true\") print(ok, code)'",
    "true\t0"
);
lua_expect!(
    lua_os_execute_false,
    "lua -e 'local ok,_,code = os.execute(\"false\") print(ok, code)'",
    "nil\t1"
);
lua_expect!(
    lua_os_execute_echo,
    "lua -e 'os.execute(\"echo from_exec\")'",
    "from_exec"
);
lua_expect!(
    lua_os_execute_nil,
    "lua -e 'local ok,_,_ = os.execute() print(ok)'",
    "true"
);

// os.remove
lua_expect!(
    lua_os_remove,
    "echo x > /tmp/rm.txt && lua -e 'print(os.remove(\"/tmp/rm.txt\"))' && test ! -f /tmp/rm.txt && echo gone",
    "true\ngone"
);

// os.rename
lua_expect!(
    lua_os_rename,
    "echo data > /tmp/rn1.txt && lua -e 'print(os.rename(\"/tmp/rn1.txt\", \"/tmp/rn2.txt\"))' && cat /tmp/rn2.txt",
    "true\ndata"
);

// os.exit
lua_status!(lua_os_exit_0, "lua -e 'os.exit(0)'", 0);
lua_status!(lua_os_exit_1, "lua -e 'os.exit(1)'", 1);
lua_status!(lua_os_exit_42, "lua -e 'os.exit(42)'", 42);
lua_status!(lua_os_exit_true, "lua -e 'os.exit(true)'", 0);
lua_status!(lua_os_exit_false, "lua -e 'os.exit(false)'", 1);
lua_status!(lua_os_exit_default, "lua -e 'os.exit()'", 0);

// ── dofile / loadfile / require ─────────────────────────────────────

lua_expect!(
    lua_dofile,
    "echo 'print(\"from dofile\")' > /tmp/df.lua && lua -e 'dofile(\"/tmp/df.lua\")'",
    "from dofile"
);
lua_expect!(
    lua_dofile_return,
    "echo 'return 42' > /tmp/dfr.lua && lua -e 'print(dofile(\"/tmp/dfr.lua\"))'",
    "42"
);
lua_expect!(
    lua_loadfile,
    "echo 'return function(x) return x*2 end' > /tmp/lf.lua && lua -e 'local f=loadfile(\"/tmp/lf.lua\") print(f()(21))'",
    "42"
);

lua_expect!(
    lua_require_module,
    "echo 'local M={} function M.greet() return \"hi\" end return M' > /tmp/mymod.lua && lua -e 'package.path=\"/tmp/?.lua\" local m=require(\"mymod\") print(m.greet())'",
    "hi"
);
lua_expect!(
    lua_require_cached,
    "echo 'return 99' > /tmp/cached.lua && lua -e 'package.path=\"/tmp/?.lua\" local a=require(\"cached\") local b=require(\"cached\") print(a,b)'",
    "99\t99"
);

// ── script file execution ───────────────────────────────────────────

lua_expect!(
    lua_script_file,
    "echo 'print(\"script\")' > /tmp/s.lua && lua /tmp/s.lua",
    "script"
);
lua_expect!(
    lua_script_args,
    "echo 'print(arg[1], arg[2])' > /tmp/sa.lua && lua /tmp/sa.lua foo bar",
    "foo\tbar"
);
lua_expect!(
    lua_script_shebang,
    "printf '#!/usr/bin/lua\\nprint(\"shebang\")' > /tmp/sh.lua && lua /tmp/sh.lua",
    "shebang"
);

// ── stdin execution ─────────────────────────────────────────────────

lua_expect!(
    lua_stdin_exec,
    "echo 'print(\"from stdin\")' | lua",
    "from stdin"
);

// ── arg table ───────────────────────────────────────────────────────

lua_expect!(
    lua_arg_n,
    "echo 'print(arg.n)' > /tmp/an.lua && lua /tmp/an.lua a b c",
    "3"
);

// ── error handling ──────────────────────────────────────────────────

lua_status!(lua_syntax_error, "lua -e 'if'", 1);
lua_status!(lua_runtime_error, "lua -e 'error(\"boom\")'", 1);
lua_stderr!(lua_runtime_error_msg, "lua -e 'error(\"boom\")'", "boom");

// ── sandbox: dangerous globals removed ──────────────────────────────

lua_expect!(lua_no_load, "lua -e 'print(type(load))'", "nil");
lua_expect!(
    lua_no_collectgarbage,
    "lua -e 'print(type(collectgarbage))'",
    "nil"
);
lua_expect!(lua_no_rawset, "lua -e 'print(type(rawset))'", "nil");
lua_expect!(lua_no_rawget, "lua -e 'print(type(rawget))'", "nil");
lua_expect!(
    lua_no_setmetatable,
    "lua -e 'print(type(setmetatable))'",
    "nil"
);
lua_expect!(
    lua_no_getmetatable,
    "lua -e 'print(type(getmetatable))'",
    "nil"
);

// ── CLI arg parsing ─────────────────────────────────────────────────

lua_status!(lua_e_missing_arg, "lua -e", 1);
lua_status!(lua_unknown_option, "lua -z", 1);
lua_expect!(
    lua_double_dash,
    "echo 'print(\"dd\")' > /tmp/dd.lua && lua -- /tmp/dd.lua",
    "dd"
);

// ── coroutine library ───────────────────────────────────────────────

lua_expect!(
    lua_coroutine_basic,
    "lua -e 'local co=coroutine.create(function() coroutine.yield(1) coroutine.yield(2) return 3 end) local _,a=coroutine.resume(co) local _,b=coroutine.resume(co) local _,c=coroutine.resume(co) print(a,b,c)'",
    "1\t2\t3"
);
lua_expect!(
    lua_coroutine_status,
    "lua -e 'local co=coroutine.create(function() coroutine.yield() end) print(coroutine.status(co)) coroutine.resume(co) print(coroutine.status(co)) coroutine.resume(co) print(coroutine.status(co))'",
    "suspended\nsuspended\ndead"
);
lua_expect!(
    lua_coroutine_wrap,
    "lua -e 'local f=coroutine.wrap(function() coroutine.yield(10) return 20 end) print(f(), f())'",
    "10\t20"
);

// ── utf8 library ────────────────────────────────────────────────────

lua_expect!(lua_utf8_len, "lua -e 'print(utf8.len(\"hello\"))'", "5");
lua_expect!(
    lua_utf8_char,
    "lua -e 'print(utf8.char(72,101,108,108,111))'",
    "Hello"
);

// ── ipairs / pairs / next / unpack / select ─────────────────────────

lua_expect!(
    lua_ipairs,
    "lua -e 'local s=\"\" for i,v in ipairs({\"a\",\"b\",\"c\"}) do s=s..i..v end print(s)'",
    "1a2b3c"
);
lua_expect!(
    lua_next,
    "lua -e 'local t={x=1} local k,v=next(t) print(k,v)'",
    "x\t1"
);
lua_expect!(lua_next_nil, "lua -e 'print(next({}))'", "nil");

// ── multiple statements ─────────────────────────────────────────────

lua_expect!(
    lua_multi_stmt,
    "lua -e 'local x=1 local y=2 print(x+y)'",
    "3"
);
lua_expect!(
    lua_local_scope,
    "lua -e 'do local x=42 end print(x)'",
    "nil"
);

// ── metatables via __index (pcall since setmetatable removed) ───────

lua_expect!(
    lua_pcall_no_setmetatable,
    "lua -e 'local ok=pcall(setmetatable, {}, {}) print(ok)'",
    "false"
);

// ── string methods via colon syntax ─────────────────────────────────

lua_expect!(
    lua_string_method_upper,
    "lua -e 'print((\"hello\"):upper())'",
    "HELLO"
);
lua_expect!(
    lua_string_method_sub,
    "lua -e 'print((\"abcdef\"):sub(2,4))'",
    "bcd"
);
lua_expect!(
    lua_string_method_rep,
    "lua -e 'print((\"x\"):rep(5))'",
    "xxxxx"
);
lua_expect!(
    lua_string_method_find,
    "lua -e 'print((\"hello\"):find(\"ll\"))'",
    "3\t4"
);

// ── io.open error paths ─────────────────────────────────────────────

lua_status!(
    lua_io_open_nonexistent,
    "lua -e 'local f,e = io.open(\"/nonexistent\") if not f then print(e) os.exit(1) end'",
    1
);

// ── io.write error: bad type ────────────────────────────────────────

lua_status!(
    lua_io_write_bad_type,
    "lua -e 'local ok,e = pcall(io.write, {}) if not ok then os.exit(2) end'",
    2
);

// ── io.lines with filename (unsupported) ────────────────────────────

lua_status!(
    lua_io_lines_filename,
    "lua -e 'local ok,e = pcall(io.lines, \"file.txt\") if not ok then os.exit(3) end'",
    3
);

// ── io.popen write mode (unsupported) ───────────────────────────────

lua_status!(
    lua_io_popen_write_mode,
    "lua -e 'local ok,e = pcall(io.popen, \"echo\", \"w\") if not ok then os.exit(4) end'",
    4
);

// ── io.open write: write integer and float ──────────────────────────

lua_expect!(
    lua_io_open_write_int,
    "lua -e 'local f=io.open(\"/tmp/wi.txt\",\"w\") f:write(42) f:close()' && cat /tmp/wi.txt",
    "42"
);
lua_expect!(
    lua_io_open_write_float,
    "lua -e 'local f=io.open(\"/tmp/wf.txt\",\"w\") f:write(3.14) f:close()' && cat /tmp/wf.txt",
    "3.14"
);

// ── io.stderr write integer ─────────────────────────────────────────

#[test]
fn lua_io_stderr_write_int() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("lua -e 'io.stderr:write(99)'").await;
        assert!(out.stderr.contains("99"), "stderr: {}", out.stderr);
    }));
}

// ── os.remove nonexistent ───────────────────────────────────────────

lua_status!(
    lua_os_remove_nonexistent,
    "lua -e 'local ok,e = pcall(os.remove, \"/nonexistent\") if not ok then os.exit(5) end'",
    5
);

// ── os.rename nonexistent ───────────────────────────────────────────

lua_status!(
    lua_os_rename_nonexistent,
    "lua -e 'local ok,e = pcall(os.rename, \"/nonexistent\", \"/tmp/x\") if not ok then os.exit(6) end'",
    6
);

// ── dofile nonexistent ──────────────────────────────────────────────

lua_status!(
    lua_dofile_nonexistent,
    "lua -e 'dofile(\"/nonexistent.lua\")'",
    1
);

// ── dofile nil (no filename) ────────────────────────────────────────

lua_status!(
    lua_dofile_nil,
    "lua -e 'local ok,e = pcall(dofile) if not ok then os.exit(7) end'",
    7
);

// ── loadfile nonexistent ────────────────────────────────────────────

lua_status!(
    lua_loadfile_nonexistent,
    "lua -e 'local ok,e = pcall(loadfile, \"/nonexistent.lua\") if not ok then os.exit(8) end'",
    8
);

// ── loadfile nil ────────────────────────────────────────────────────

lua_status!(
    lua_loadfile_nil,
    "lua -e 'local ok,e = pcall(loadfile) if not ok then os.exit(9) end'",
    9
);

// ── require nonexistent ─────────────────────────────────────────────

lua_status!(
    lua_require_nonexistent,
    "lua -e 'local ok,e = pcall(require, \"nonexistent_module_xyz\") if not ok then os.exit(10) end'",
    10
);

// ── require returns nil → stored as true ────────────────────────────

lua_expect!(
    lua_require_nil_module,
    "echo 'return nil' > /tmp/nilmod.lua && lua -e 'package.path=\"/tmp/?.lua\" local a=require(\"nilmod\") print(type(a))'",
    "nil"
);

// ── script file nonexistent ─────────────────────────────────────────

lua_status!(lua_script_nonexistent, "lua /nonexistent.lua", 1);

// ── -- with no file after ───────────────────────────────────────────

lua_expect!(
    lua_double_dash_no_file,
    "echo 'print(\"ok\")' | lua --",
    "ok"
);

// ── read_cursor *a format ───────────────────────────────────────────

lua_expect!(
    lua_io_read_a_short,
    "echo 'hello' | lua -e 'print(io.read(\"a\"))'",
    "hello"
);
lua_expect!(
    lua_io_read_l_short,
    "echo 'hello' | lua -e 'print(io.read(\"l\"))'",
    "hello"
);
lua_expect!(
    lua_io_read_n_short,
    "echo '3.14' | lua -e 'print(io.read(\"n\"))'",
    "3.14"
);

// ── read_cursor unsupported format ──────────────────────────────────

lua_status!(
    lua_io_read_bad_format,
    "echo 'x' | lua -e 'local ok=pcall(io.read, \"*z\") if not ok then os.exit(11) end'",
    11
);

// ── read past EOF returns nil ───────────────────────────────────────

lua_expect!(
    lua_io_read_eof,
    "printf '' | lua -e 'local v=io.read(\"*l\") if v==nil then print(\"nil\") else print(v) end'",
    "nil"
);

// ── io.open read: read *a ───────────────────────────────────────────

lua_expect!(
    lua_io_open_read_all,
    "echo 'content' > /tmp/ra.txt && lua -e 'local f=io.open(\"/tmp/ra.txt\") print(f:read(\"*a\"))'",
    "content"
);

// ── io.open read: read *n ───────────────────────────────────────────

lua_expect!(
    lua_io_open_read_number,
    "echo '99' > /tmp/rn.txt && lua -e 'local f=io.open(\"/tmp/rn.txt\") print(f:read(\"*n\"))'",
    "99"
);

// ── io.close standalone ─────────────────────────────────────────────

lua_expect!(lua_io_close, "lua -e 'io.close() print(\"ok\")'", "ok");

// ── val_to_string fallback ──────────────────────────────────────────

// val_to_string handles tables via tostring
#[test]
fn lua_print_table_type() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("lua -e 'print({})'").await;
        assert!(out.stdout.starts_with("table: "), "stdout: {}", out.stdout);
        assert_eq!(out.status, 0);
    }));
}

// ── os.execute captures stderr ──────────────────────────────────────

#[test]
fn lua_os_execute_stderr() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("lua -e 'os.execute(\"echo err >&2\")'").await;
        assert!(out.stderr.contains("err"), "stderr: {}", out.stderr);
    }));
}

// ── os.exit with number as float ────────────────────────────────────

lua_status!(lua_os_exit_float, "lua -e 'os.exit(2.5)'", 2);

// ── io.open write: bad type ─────────────────────────────────────────

lua_status!(
    lua_io_open_write_bad_type,
    "lua -e 'local f=io.open(\"/tmp/wb.txt\",\"w\") local ok=pcall(f.write, f, {}) if not ok then os.exit(12) end'",
    12
);

// ── io.stderr write: bad type ───────────────────────────────────────

lua_status!(
    lua_io_stderr_write_bad_type,
    "lua -e 'local ok=pcall(io.stderr.write, io.stderr, {}) if not ok then os.exit(13) end'",
    13
);

// ── shebang in file ─────────────────────────────────────────────────

lua_expect!(
    lua_dofile_shebang,
    "printf '#!/usr/bin/lua\\nreturn 77' > /tmp/shb.lua && lua -e 'print(dofile(\"/tmp/shb.lua\"))'",
    "77"
);

// ── loadfile with shebang ───────────────────────────────────────────

lua_expect!(
    lua_loadfile_shebang,
    "printf '#!/usr/bin/lua\\nreturn 88' > /tmp/lfs.lua && lua -e 'print(loadfile(\"/tmp/lfs.lua\")())'",
    "88"
);

// ── require with dotted name ────────────────────────────────────────

lua_expect!(
    lua_require_dotted,
    "mkdir -p /tmp/luamods/sub && echo 'return 55' > /tmp/luamods/sub/init.lua && lua -e 'package.path=\"/tmp/luamods/?.lua;/tmp/luamods/?/init.lua\" print(require(\"sub\"))'",
    "55"
);

// ── multiple io.read calls ──────────────────────────────────────────

lua_expect!(
    lua_io_read_multi_lines,
    "printf 'a\\nb\\nc\\n' | lua -e 'print(io.read(), io.read(), io.read())'",
    "a\tb\tc"
);

// ── io.open file handle close ───────────────────────────────────────

lua_expect!(
    lua_io_open_close,
    "echo x > /tmp/cl.txt && lua -e 'local f=io.open(\"/tmp/cl.txt\") f:close() print(\"ok\")'",
    "ok"
);

// ── timeout / deadline ──────────────────────────────────────────────

#[test]
fn lua_timeout_exceeded() {
    use std::time::Duration;
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let out = shell.run("lua -e 'while true do end'").await;
        assert_ne!(out.status, 0, "should fail with timeout");
        assert!(out.stderr.contains("timeout"), "stderr: {}", out.stderr);
    }));
}

// ── io.open write to nonexistent dir ────────────────────────────────

lua_status!(
    lua_io_open_write_bad_path,
    "lua -e 'local ok,e = pcall(function() local f=io.open(\"/no/such/dir/f.txt\",\"w\") f:write(\"x\") f:close() end) if not ok then os.exit(14) end'",
    14
);

// ── require with dot path (sub.mod → sub/mod.lua) ──────────────────

lua_expect!(
    lua_require_dot_path,
    "mkdir -p /tmp/luadot/sub && echo 'return 77' > /tmp/luadot/sub/mod.lua && lua -e 'package.path=\"/tmp/luadot/?.lua\" print(require(\"sub.mod\"))'",
    "77"
);

// ── require cached returns true for nil module on second call ───────

lua_expect!(
    lua_require_nil_cached,
    "echo 'return nil' > /tmp/nilcache.lua && lua -e 'package.path=\"/tmp/?.lua\" require(\"nilcache\") print(type(require(\"nilcache\")))'",
    "boolean"
);

// ── io.read *n with non-number ──────────────────────────────────────

lua_expect!(
    lua_io_read_n_nan,
    "echo 'abc' | lua -e 'local v=io.read(\"*n\") print(v)'",
    "nil"
);

// ── io.open read: lines on empty file ───────────────────────────────

lua_expect!(
    lua_io_open_lines_empty,
    "printf '' > /tmp/empty.txt && lua -e 'local t={} local f=io.open(\"/tmp/empty.txt\") for l in f:lines() do t[#t+1]=l end print(#t)'",
    "0"
);

// ── multiple print calls ────────────────────────────────────────────

lua_expect!(
    lua_multi_print,
    "lua -e 'print(\"a\") print(\"b\") print(\"c\")'",
    "a\nb\nc"
);

// ── string.format edge cases ────────────────────────────────────────

lua_expect!(
    lua_string_format_pct,
    "lua -e 'print(string.format(\"100%%\"))'",
    "100%"
);
lua_expect!(
    lua_string_format_float,
    "lua -e 'print(string.format(\"%.2f\", 3.14159))'",
    "3.14"
);

// ── nested function calls ───────────────────────────────────────────

lua_expect!(
    lua_nested_calls,
    "lua -e 'print(tostring(tonumber(\"42\")))'",
    "42"
);

// ── empty script ────────────────────────────────────────────────────

lua_expect!(lua_empty_script, "lua -e ''", "");

// ── multiple -e not supported (only last one) ───────────────────────

lua_expect!(
    lua_e_override,
    "lua -e 'x=1' -e 'print(x or \"nil\")'",
    "nil"
);

// ── io.open read then read past EOF ─────────────────────────────────

lua_expect!(
    lua_io_open_read_eof,
    "echo 'x' > /tmp/eof.txt && lua -e 'local f=io.open(\"/tmp/eof.txt\") f:read(\"*l\") print(f:read(\"*l\"))'",
    "nil"
);

// ── os.execute with pipeline ────────────────────────────────────────

lua_expect!(
    lua_os_execute_pipe,
    "lua -e 'os.execute(\"echo hello | tr a-z A-Z\")'",
    "HELLO"
);

// ── large output ────────────────────────────────────────────────────

lua_expect!(
    lua_large_output,
    "lua -e 'for i=1,100 do io.write(\"x\") end print()'",
    "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
);

// ── string.byte with range ──────────────────────────────────────────

lua_expect!(
    lua_string_byte_range,
    "lua -e 'print(string.byte(\"ABC\", 1, 3))'",
    "65\t66\t67"
);

// ── table.concat with separator and range ───────────────────────────

lua_expect!(
    lua_table_concat_range,
    "lua -e 'print(table.concat({\"a\",\"b\",\"c\",\"d\"}, \"-\", 2, 3))'",
    "b-c"
);

// ── math.random (just check it returns a number) ────────────────────

lua_expect!(
    lua_math_random_type,
    "lua -e 'print(type(math.random()))'",
    "number"
);

// ── pcall with non-function ─────────────────────────────────────────

lua_expect!(
    lua_pcall_non_func,
    "lua -e 'local ok,e = pcall(42) print(ok)'",
    "false"
);

// ── multiple return from dofile ─────────────────────────────────────

lua_expect!(
    lua_dofile_multi_return,
    "echo 'return 1,2,3' > /tmp/dmr.lua && lua -e 'print(dofile(\"/tmp/dmr.lua\"))'",
    "1\t2\t3"
);

// ── loadfile syntax error ───────────────────────────────────────────

lua_status!(
    lua_loadfile_syntax_error,
    "echo 'if' > /tmp/syn.lua && lua -e 'local ok,e = pcall(loadfile, \"/tmp/syn.lua\") if not ok then os.exit(15) end'",
    15
);

// ── require syntax error in module ──────────────────────────────────

lua_status!(
    lua_require_syntax_error,
    "echo 'if' > /tmp/synerr.lua && lua -e 'package.path=\"/tmp/?.lua\" local ok,e = pcall(require, \"synerr\") if not ok then os.exit(16) end'",
    16
);

// ── REPL tests ──────────────────────────────────────────────────────

use std::cell::RefCell;
use std::rc::Rc;

fn repl_test(lines: &[&str]) -> (String, String, i32) {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let shell = Shell::builder().build().unwrap();
        let kernel = shell.kernel().clone();
        let stdout_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let stderr_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let proc_cell = RefCell::new(shell.proc);
        let sb = stdout_buf.clone();
        let eb = stderr_buf.clone();
        let lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        let (out, err, code) = strands_shell::io::CURRENT_KERNEL
            .scope(
                kernel,
                strands_shell::io::CURRENT_PROCESS.scope(proc_cell, async move {
                    let lua = strands_shell::io::with_process(|p| {
                        strands_shell::builtins::lua::setup_lua_vm(p, &[], "", &sb, &eb)
                    })
                    .unwrap();
                    let idx = RefCell::new(0usize);
                    let mut read_line = |_prompt: &str| -> Option<String> {
                        let mut i = idx.borrow_mut();
                        if *i < lines.len() {
                            let line = lines[*i].clone();
                            *i += 1;
                            Some(line)
                        } else {
                            None
                        }
                    };
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    let code = strands_shell::builtins::lua::repl_loop(
                        &lua,
                        &mut read_line,
                        &mut out,
                        &mut err,
                        &sb,
                        &eb,
                    )
                    .await;
                    (
                        String::from_utf8(out).unwrap(),
                        String::from_utf8(err).unwrap(),
                        code,
                    )
                }),
            )
            .await;
        (out, err, code)
    }))
}

#[test]
fn repl_expression() {
    let (out, _, code) = repl_test(&["1+2"]);
    assert_eq!(out.trim(), "3");
    assert_eq!(code, 0);
}

#[test]
fn repl_string_expr() {
    let (out, _, _) = repl_test(&["\"hello\""]);
    assert_eq!(out.trim(), "hello");
}

#[test]
fn repl_statement() {
    let (out, _, _) = repl_test(&["print(42)"]);
    assert_eq!(out.trim(), "42");
}

#[test]
fn repl_variable_persistence() {
    let (out, _, _) = repl_test(&["x = 10", "x * 3"]);
    assert_eq!(out.trim(), "30");
}

#[test]
fn repl_multiline_function() {
    let (out, _, _) = repl_test(&["function foo()", "return 99", "end", "foo()"]);
    assert_eq!(out.trim(), "99");
}

#[test]
fn repl_multiline_for() {
    let (out, _, _) = repl_test(&["for i=1,3 do", "print(i)", "end"]);
    assert_eq!(out.trim(), "1\n2\n3");
}

#[test]
fn repl_syntax_error_recovery() {
    let (out, err, _) = repl_test(&["bad syntax %%", "print(\"ok\")"]);
    assert!(
        err.contains("syntax error"),
        "expected syntax error in: {err}"
    );
    assert_eq!(out.trim(), "ok");
}

#[test]
fn repl_runtime_error_recovery() {
    let (out, err, _) = repl_test(&["error(\"boom\")", "print(\"ok\")"]);
    assert!(err.contains("boom"), "expected boom in: {err}");
    assert_eq!(out.trim(), "ok");
}

#[test]
fn repl_os_exit() {
    let (_, _, code) = repl_test(&["os.exit(0)"]);
    assert_eq!(code, 0);
}

#[test]
fn repl_nil_not_printed() {
    let (out, _, _) = repl_test(&["nil"]);
    assert_eq!(out.trim(), "");
}

#[test]
fn repl_multiple_return() {
    let (out, _, _) = repl_test(&["1, 2, 3"]);
    assert_eq!(out.trim(), "1\t2\t3");
}

#[test]
fn repl_math_stdlib() {
    let (out, _, _) = repl_test(&["math.floor(3.7)"]);
    assert_eq!(out.trim(), "3");
}

#[test]
fn repl_string_stdlib() {
    let (out, _, _) = repl_test(&["string.upper(\"abc\")"]);
    assert_eq!(out.trim(), "ABC");
}

#[test]
fn repl_multiline_if() {
    let (out, _, _) = repl_test(&["if true then", "print(\"yes\")", "end"]);
    assert_eq!(out.trim(), "yes");
}

#[test]
fn repl_empty_eof() {
    let (out, _, code) = repl_test(&[]);
    assert_eq!(out.trim(), "");
    assert_eq!(code, 0);
}

#[test]
fn repl_table_constructor() {
    let (out, _, _) = repl_test(&["t = {10,20,30}", "t[2]"]);
    assert_eq!(out.trim(), "20");
}

#[test]
fn repl_boolean_expr() {
    let (out, _, _) = repl_test(&["true"]);
    assert_eq!(out.trim(), "true");
}

#[test]
fn repl_multiline_while() {
    let (out, _, _) = repl_test(&["x=0", "while x<3 do", "x=x+1", "end", "x"]);
    assert_eq!(out.trim(), "3");
}

#[test]
fn repl_multiline_nested() {
    let (out, _, _) = repl_test(&[
        "function outer()",
        "function inner()",
        "return 7",
        "end",
        "return inner()",
        "end",
        "outer()",
    ]);
    assert_eq!(out.trim(), "7");
}

#[test]
fn repl_print_with_expression() {
    // print() goes through sandbox stdout_buf, expression goes through out writer
    let (out, _, _) = repl_test(&["print(\"a\")", "\"b\""]);
    assert_eq!(out.trim(), "a\nb");
}

#[test]
fn repl_multiline_eof_mid_input() {
    // EOF while accumulating multi-line input should not crash
    let (_, _, code) = repl_test(&["function foo()"]);
    assert_eq!(code, 0);
}

#[test]
fn repl_concat_expr() {
    let (out, _, _) = repl_test(&["\"hello\" .. \" \" .. \"world\""]);
    assert_eq!(out.trim(), "hello world");
}

#[test]
fn repl_local_var() {
    // local variables are scoped to the chunk, so not visible in next line
    let (out, err, _) = repl_test(&["local x = 5", "print(x)"]);
    assert_eq!(out.trim(), "nil");
    assert!(err.is_empty());
}

#[test]
fn lua_memory_limit_prevents_exhaustion() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        // Attempt to allocate well over the 100MB limit
        let out = shell.run("lua -e 'x = string.rep(\"A\", 200000000)'").await;
        assert_ne!(
            out.status, 0,
            "large allocation should fail; stderr: {}",
            out.stderr
        );
    }));
}
