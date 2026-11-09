//! A custom kernel that runs commands Shell does not implement.
//!
//! `HostKernel` forwards every operation to the default kernel and adds a
//! [`CommandFallback`]. The tests drive it through `Shell::with_kernel` and
//! `Shell::run`, with in-memory handlers in place of real programs.

use std::future::{Future, pending};
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use strands_shell::Shell;
use strands_shell::commands::{CommandEntry, CommandResult};
use strands_shell::os::{
    CommandContext, CommandFallback, DirEntry, Fd, FileStat, HttpRequest, HttpResponse, Kernel,
    OpenFlags, Process,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

type HandlerFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Option<i32>>> + Send + 'a>>;
type Handler = dyn for<'a> Fn(&'a mut CommandContext) -> HandlerFuture<'a> + Send + Sync;

struct HostKernel {
    inner: Arc<dyn Kernel>,
    handler: Box<Handler>,
}

#[async_trait]
impl CommandFallback for HostKernel {
    async fn run(&self, ctx: &mut CommandContext) -> io::Result<Option<i32>> {
        (self.handler)(ctx).await
    }
}

#[async_trait]
impl Kernel for HostKernel {
    fn command_fallback(&self) -> Option<&dyn CommandFallback> {
        Some(self)
    }

    fn new_process(&self) -> Process {
        self.inner.new_process()
    }

    async fn open(&self, proc: &mut Process, path: &str, flags: OpenFlags) -> io::Result<Fd> {
        self.inner.open(proc, path, flags).await
    }

    async fn list_dir(&self, proc: &Process, path: &str) -> io::Result<Vec<DirEntry>> {
        self.inner.list_dir(proc, path).await
    }

    async fn change_dir(&self, proc: &mut Process, path: &str) -> io::Result<()> {
        self.inner.change_dir(proc, path).await
    }

    async fn stat(&self, proc: &Process, path: &str) -> FileStat {
        self.inner.stat(proc, path).await
    }

    async fn lstat(&self, proc: &Process, path: &str) -> FileStat {
        self.inner.lstat(proc, path).await
    }

    async fn access(&self, proc: &Process, path: &str, mode: i32) -> bool {
        self.inner.access(proc, path, mode).await
    }

    async fn canonicalize(&self, proc: &Process, path: &str) -> io::Result<PathBuf> {
        self.inner.canonicalize(proc, path).await
    }

    async fn is_executable(&self, proc: &Process, path: &str) -> bool {
        self.inner.is_executable(proc, path).await
    }

    async fn glob(&self, proc: &Process, pattern: &str) -> Vec<String> {
        self.inner.glob(proc, pattern).await
    }

    fn isatty(&self, fd: i32) -> bool {
        self.inner.isatty(fd)
    }

    async fn remove_file(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.remove_file(proc, path).await
    }

    async fn remove_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.remove_dir(proc, path).await
    }

    async fn create_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.create_dir(proc, path).await
    }

    async fn rename(&self, proc: &Process, from: &str, to: &str) -> io::Result<()> {
        self.inner.rename(proc, from, to).await
    }

    async fn symlink(&self, proc: &Process, target: &str, link: &str) -> io::Result<()> {
        self.inner.symlink(proc, target, link).await
    }

    async fn read_link(&self, proc: &Process, path: &str) -> io::Result<String> {
        self.inner.read_link(proc, path).await
    }

    async fn set_permissions(&self, proc: &Process, path: &str, mode: u32) -> io::Result<()> {
        self.inner.set_permissions(proc, path, mode).await
    }

    fn now(&self) -> SystemTime {
        self.inner.now()
    }

    fn check_url(&self, url: &str) -> io::Result<()> {
        self.inner.check_url(url)
    }

    fn resolve_credential(&self, url: &str, method: &str) -> Vec<(String, String)> {
        self.inner.resolve_credential(url, method)
    }

    async fn http_request(&self, req: HttpRequest) -> io::Result<HttpResponse> {
        self.inner.http_request(req).await
    }
}

fn host_shell<F>(handler: F) -> Shell
where
    F: for<'a> Fn(&'a mut CommandContext) -> HandlerFuture<'a> + Send + Sync + 'static,
{
    let inner = Shell::builder().build().unwrap().kernel().clone();
    Shell::with_kernel(Arc::new(HostKernel {
        inner,
        handler: Box::new(handler),
    }))
}

fn block_on<F: Future>(future: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&rt, future)
}

async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

#[derive(Clone, Debug)]
struct Invocation {
    program: String,
    args: Vec<String>,
    cwd: String,
    mode: Option<String>,
}

/// Sets a flag while a handler future is alive.
struct Active(Arc<AtomicBool>);

impl Active {
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(flag)
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Fake `git` and `cargo` programs. Every other name declines.
fn fake_programs(ctx: &mut CommandContext) -> HandlerFuture<'_> {
    Box::pin(async move {
        let program = ctx.program().to_string();
        let args = ctx.args().to_vec();
        let (stdin, stdout, stderr) = ctx.stdio();
        if program == "git" {
            stdout.unwrap().write_all(b"deadbeef\n").await?;
            return Ok(Some(0));
        }
        if program != "cargo" {
            return Ok(None);
        }
        match args.first().map(String::as_str) {
            Some("run") => {
                if let (Some(stdin), Some(stdout)) = (stdin, stdout) {
                    tokio::io::copy(stdin, stdout).await?;
                }
                Ok(Some(0))
            }
            Some("build") | Some("test") => {
                if let Some(stdout) = stdout {
                    stdout
                        .write_all(format!("{}\n", args[0]).as_bytes())
                        .await?;
                }
                Ok(Some(0))
            }
            Some("bytes") => {
                let stdout = stdout.unwrap();
                for _ in 0..128 {
                    stdout.write_all(&[0, 255, 97, 10]).await?;
                }
                Ok(Some(0))
            }
            Some("noisy-copy") => {
                let stderr = stderr.unwrap();
                for _ in 0..128 {
                    stderr.write_all(b"e").await?;
                }
                tokio::io::copy(stdin.unwrap(), stdout.unwrap()).await?;
                Ok(Some(0))
            }
            Some("producer") => {
                let stdout = stdout.unwrap();
                for _ in 0..1024 {
                    stdout.write_all(b"x").await?;
                }
                Ok(Some(0))
            }
            Some("emit") => {
                if let Some(stdout) = stdout {
                    stdout.write_all(b"OUT\n").await?;
                }
                if let Some(stderr) = stderr {
                    stderr.write_all(b"ERR\n").await?;
                }
                Ok(Some(0))
            }
            Some("bad") => Ok(Some(9)),
            Some("status127") => Ok(Some(127)),
            Some("error") => Err(io::Error::other("runner failed")),
            _ => Ok(Some(0)),
        }
    })
}

fn recording_shell(calls: Arc<Mutex<Vec<Invocation>>>) -> Shell {
    host_shell(move |ctx| {
        calls.lock().unwrap().push(Invocation {
            program: ctx.program().to_string(),
            args: ctx.args().to_vec(),
            cwd: ctx.cwd().to_string_lossy().into_owned(),
            mode: ctx.env().get("MODE").cloned(),
        });
        fake_programs(ctx)
    })
}

#[test]
fn the_issue_examples_run_inside_the_script() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        let cases = [
            ("echo hello | cargo run", 0, "hello\n"),
            ("cargo build && cargo test", 0, "build\ntest\n"),
            ("x=$(git rev-parse HEAD); printf '%s' \"$x\"", 0, "deadbeef"),
            ("(cargo build)", 0, "build\n"),
            ("f() { cargo build; }; f", 0, "build\n"),
            ("printf 'x\\n' | xargs cargo build", 0, "build\n"),
            ("command cargo build", 0, "build\n"),
        ];
        for (script, status, stdout) in cases {
            let out = shell.run(script).await;
            assert_eq!(out.status, status, "{script}: {}", out.stderr);
            assert_eq!(out.stdout, stdout, "{script}");
            assert_eq!(out.stderr, "", "{script}");
        }

        shell.write_file("/tmp/out.txt", b"old").await.unwrap();
        let before = calls.lock().unwrap().len();
        let out = shell.run("rm /tmp/out.txt; cargo build").await;
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "build\n");
        assert_eq!(calls.lock().unwrap().len(), before + 1);
        assert!(shell.read_file("/tmp/out.txt").await.is_err());
    });
}

#[test]
fn a_returned_127_runs_the_handler_once() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        let out = shell.run("cargo status127; echo $?").await;
        assert_eq!(out.stdout, "127\n");
        assert_eq!(out.stderr, "");
        assert_eq!(calls.lock().unwrap().len(), 1);
    });
}

#[test]
fn closed_or_absent_descriptors_stay_unavailable() {
    block_on(async {
        let mut shell = host_shell(|ctx| {
            Box::pin(async move {
                let (stdin, stdout, stderr) = ctx.stdio();
                let seen = format!(
                    "{} {} {}\n",
                    stdin.is_some(),
                    stdout.is_some(),
                    stderr.is_some()
                );
                if let Some(stderr) = stderr {
                    stderr.write_all(seen.as_bytes()).await?;
                }
                Ok(Some(0))
            })
        });
        let cases = [
            ("probe", "false true true\n"),
            ("echo x | probe", "true true true\n"),
            ("true | probe <&-", "false true true\n"),
            ("probe >&-", "false false true\n"),
        ];
        for (script, stderr) in cases {
            let out = shell.run(script).await;
            assert_eq!(out.status, 0, "{script}");
            assert_eq!(out.stderr, stderr, "{script}");
        }
        assert_eq!(shell.run("probe 2>&-").await.stderr, "");
    });
}

#[test]
fn a_handled_status_drives_control_flow() {
    block_on(async {
        let mut shell = recording_shell(Arc::new(Mutex::new(Vec::new())));
        let cases = [
            ("cargo bad && echo wrong", 9, ""),
            ("cargo bad || echo recovered", 0, "recovered\n"),
            ("cargo bad; echo $?", 0, "9\n"),
            ("cargo status127", 127, ""),
            ("set -e; cargo bad; echo wrong", 9, ""),
        ];
        for (script, status, stdout) in cases {
            let out = shell.run(script).await;
            assert_eq!(out.status, status, "{script}");
            assert_eq!(out.stdout, stdout, "{script}");
            assert_eq!(out.stderr, "", "{script}");
        }
    });
}

#[test]
fn the_handler_sees_expanded_args_cwd_and_command_local_env() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        let out = shell
            .run(
                "mkdir /tmp/work; cd /tmp/work; CMD=cargo; MODE=safe $CMD context 'a b' '$literal'",
            )
            .await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        let call = calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(call.program, "cargo");
        assert_eq!(call.args, ["context", "a b", "$literal"]);
        assert_eq!(call.cwd, "/tmp/work");
        assert_eq!(call.mode.as_deref(), Some("safe"));
        assert_eq!(shell.get_env("MODE"), None);
    });
}

#[test]
fn a_decline_matches_a_shell_without_a_handler() {
    block_on(async {
        let mut plain = Shell::builder().build().unwrap();
        let mut shell = recording_shell(Arc::new(Mutex::new(Vec::new())));
        for script in [
            "missing",
            "missing arg; echo $?",
            "missing 2>&1",
            "missing 2>&-",
        ] {
            let baseline = plain.run(script).await;
            let declined = shell.run(script).await;
            assert_eq!(declined.status, baseline.status, "{script}");
            assert_eq!(declined.stdout, baseline.stdout, "{script}");
            assert_eq!(declined.stderr, baseline.stderr, "{script}");
        }
        let out = shell.run("missing").await;
        assert_eq!(out.status, 127);
        assert_eq!(out.stderr, "strands-shell: missing: command not found\n");
    });
}

#[test]
fn diagnostics_and_output_follow_redirection() {
    block_on(async {
        let mut shell = recording_shell(Arc::new(Mutex::new(Vec::new())));
        let out = shell.run("missing 2>/tmp/missing").await;
        assert_eq!(out.status, 127);
        assert_eq!(out.stderr, "");
        assert_eq!(
            shell.read_file("/tmp/missing").await.unwrap(),
            b"strands-shell: missing: command not found\n"
        );

        let out = shell.run("cargo error").await;
        assert_eq!(out.status, 1);
        assert_eq!(out.stderr, "strands-shell: cargo: runner failed\n");

        let out = shell.run("cargo error 2>/tmp/failure").await;
        assert_eq!(out.status, 1);
        assert_eq!(out.stderr, "");
        assert_eq!(
            shell.read_file("/tmp/failure").await.unwrap(),
            b"strands-shell: cargo: runner failed\n"
        );

        let out = shell.run("cargo emit >/tmp/out 2>/tmp/err").await;
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "");
        assert_eq!(out.stderr, "");
        assert_eq!(shell.read_file("/tmp/out").await.unwrap(), b"OUT\n");
        assert_eq!(shell.read_file("/tmp/err").await.unwrap(), b"ERR\n");

        let out = shell.run("cargo emit 2>&1 >/dev/null").await;
        assert_eq!(out.stdout, "ERR\n");
        assert_eq!(out.stderr, "");
    });
}

#[test]
fn closed_stderr_stays_closed_after_an_error() {
    block_on(async {
        let mut shell = host_shell(|ctx| {
            Box::pin(async move {
                let (_, _, stderr) = ctx.stdio();
                stderr.unwrap().shutdown().await?;
                Err(io::Error::other("runner failed after closing stderr"))
            })
        });
        let out = shell.run("failure").await;
        assert_eq!(out.status, 1);
        assert_eq!(out.stdout, "");
        assert_eq!(out.stderr, "");
    });
}

#[test]
fn existing_resolution_wins_over_the_handler() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        shell
            .write_file("/tmp/script", b"#!/bin/sh\necho from script\n")
            .await
            .unwrap();
        let cases = [
            ("echo builtin", "builtin\n"),
            ("printf 'a\\n' | cat", "a\n"),
            (
                "cargo() { echo function; }; cargo; unset -f cargo",
                "function\n",
            ),
            (
                "mkdir -p /tmp/bin; ln -s /bin/lash /tmp/bin/echo; /tmp/bin/echo multicall",
                "multicall\n",
            ),
            ("chmod +x /tmp/script; /tmp/script", "from script\n"),
        ];
        for (script, stdout) in cases {
            let out = shell.run(script).await;
            assert_eq!(out.status, 0, "{script}: {}", out.stderr);
            assert_eq!(out.stdout, stdout, "{script}");
        }
        assert!(calls.lock().unwrap().is_empty());
    });
}

#[test]
fn lookups_and_argument_errors_do_not_reach_the_handler() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        let mut plain = Shell::builder().build().unwrap();
        for script in ["type cargo", "command -v cargo", "cat --no-such-option"] {
            let baseline = plain.run(script).await;
            let out = shell.run(script).await;
            assert_ne!(out.status, 0, "{script}");
            assert_eq!(out.status, baseline.status, "{script}");
            assert_eq!(out.stderr, baseline.stderr, "{script}");
        }
        assert!(calls.lock().unwrap().is_empty());
    });
}

#[test]
fn a_shebang_interpreter_reaches_the_handler_with_its_full_name() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        shell
            .write_file("/tmp/tool", b"#!/usr/bin/python -u\nprint('hi')\n")
            .await
            .unwrap();
        shell.run("chmod +x /tmp/tool").await;
        let out = shell.run("/tmp/tool extra").await;
        assert_eq!(out.status, 127);
        let call = calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(call.program, "/usr/bin/python");
        assert_eq!(call.args, ["-u", "/tmp/tool", "extra"]);
    });
}

#[test]
fn nested_lua_and_a_shared_kernel_reach_the_handler() {
    block_on(async {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut shell = recording_shell(calls.clone());
        let out = shell.run("lua -e 'os.execute(\"cargo build\")'").await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "build\n");
        let mut another = Shell::with_kernel(shell.kernel().clone());
        assert_eq!(another.run("cargo build").await.stdout, "build\n");
        assert_eq!(calls.lock().unwrap().len(), 2);
    });
}

#[test]
fn binary_streams_and_early_close_complete() {
    block_on(async {
        let mut shell = recording_shell(Arc::new(Mutex::new(Vec::new())));
        let out = tokio::time::timeout(
            Duration::from_secs(5),
            shell.run("cargo bytes | cargo noisy-copy | wc -c"),
        )
        .await
        .expect("streaming pipeline deadlocked");
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout.trim(), "512");
        assert_eq!(out.stderr, "e".repeat(128));

        let out = tokio::time::timeout(
            Duration::from_secs(5),
            shell.run("cargo bytes | cargo run > /tmp/bytes"),
        )
        .await
        .unwrap();
        assert_eq!(out.status, 0);
        assert_eq!(
            shell.read_file("/tmp/bytes").await.unwrap(),
            [0u8, 255, 97, 10].repeat(128)
        );

        let out = tokio::time::timeout(
            Duration::from_secs(5),
            shell.run("cargo producer | head -c 1"),
        )
        .await
        .expect("producer did not observe downstream close");
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "x");
    });
}

#[test]
fn a_slow_reader_applies_backpressure() {
    block_on(async {
        let written = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(AtomicUsize::new(0));
        let (w, s) = (written.clone(), seen.clone());
        let mut shell = host_shell(move |ctx| {
            let (written, seen) = (w.clone(), s.clone());
            Box::pin(async move {
                let program = ctx.program().to_string();
                let (stdin, stdout, _) = ctx.stdio();
                match program.as_str() {
                    "producer" => {
                        let stdout = stdout.unwrap();
                        for _ in 0..1000 {
                            stdout.write_all(b"x").await?;
                            written.fetch_add(1, Ordering::SeqCst);
                        }
                        Ok(Some(0))
                    }
                    "consumer" => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        seen.store(written.load(Ordering::SeqCst), Ordering::SeqCst);
                        let mut input = Vec::new();
                        stdin.unwrap().read_to_end(&mut input).await?;
                        stdout
                            .unwrap()
                            .write_all(format!("{}\n", input.len()).as_bytes())
                            .await?;
                        Ok(Some(0))
                    }
                    _ => Ok(None),
                }
            })
        });
        let out = shell.run("producer | consumer").await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "1000\n");
        let before_reading = seen.load(Ordering::SeqCst);
        assert!(
            before_reading < 1000,
            "producer was not throttled: {before_reading}"
        );
    });
}

#[test]
fn stdout_shutdown_delivers_eof_before_the_handler_returns() {
    block_on(async {
        let observed_eof = Arc::new(Notify::new());
        let mut shell = host_shell(move |ctx| {
            let observed_eof = observed_eof.clone();
            Box::pin(async move {
                let program = ctx.program().to_string();
                let (stdin, stdout, _) = ctx.stdio();
                match program.as_str() {
                    "producer" => {
                        let stdout = stdout.unwrap();
                        stdout.write_all(b"payload").await?;
                        stdout.shutdown().await?;
                        stdout.shutdown().await?;
                        let late = stdout.write_all(b"late").await.unwrap_err();
                        assert_eq!(late.kind(), io::ErrorKind::BrokenPipe);
                        observed_eof.notified().await;
                        Ok(Some(0))
                    }
                    "consumer" => {
                        let mut bytes = Vec::new();
                        stdin.unwrap().read_to_end(&mut bytes).await?;
                        assert_eq!(bytes, b"payload");
                        observed_eof.notify_one();
                        stdout.unwrap().write_all(b"observed EOF\n").await?;
                        Ok(Some(0))
                    }
                    _ => Ok(None),
                }
            })
        });
        let out = tokio::time::timeout(Duration::from_secs(5), shell.run("producer | consumer"))
            .await
            .expect("stdout shutdown did not deliver EOF before the handler returned");
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "observed EOF\n");
        assert_eq!(out.stderr, "");
    });
}

#[test]
fn stdin_close_releases_the_writer_before_the_handler_returns() {
    block_on(async {
        let observed_close = Arc::new(Notify::new());
        let mut shell = host_shell(move |ctx| {
            let observed_close = observed_close.clone();
            Box::pin(async move {
                let program = ctx.program().to_string();
                let (stdin, stdout, _) = ctx.stdio();
                match program.as_str() {
                    "producer" => {
                        let stdout = stdout.unwrap();
                        loop {
                            if let Err(error) = stdout.write_all(b"x").await {
                                assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                                observed_close.notify_one();
                                return Ok(Some(0));
                            }
                        }
                    }
                    "consumer" => {
                        let stdin = stdin.unwrap();
                        stdin.close();
                        stdin.close();
                        let mut bytes = Vec::new();
                        assert_eq!(stdin.read_to_end(&mut bytes).await?, 0);
                        observed_close.notified().await;
                        stdout.unwrap().write_all(b"observed close\n").await?;
                        Ok(Some(0))
                    }
                    _ => Ok(None),
                }
            })
        });
        let out = tokio::time::timeout(Duration::from_secs(5), shell.run("producer | consumer"))
            .await
            .expect("closing stdin did not release the producer before the handler returned");
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "observed close\n");
        assert_eq!(out.stderr, "");
    });
}

#[test]
fn the_deadline_drops_the_handler_and_returns_one() {
    block_on(async {
        let active = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let seen_deadline = Arc::new(Mutex::new(None));
        let (flag, start, seen) = (active.clone(), started.clone(), seen_deadline.clone());
        let mut shell = host_shell(move |ctx| {
            let (flag, start) = (flag.clone(), start.clone());
            *seen.lock().unwrap() = ctx.deadline();
            Box::pin(async move {
                let _active = Active::new(flag);
                start.store(true, Ordering::SeqCst);
                pending::<()>().await;
                Ok(Some(0))
            })
        });
        let deadline = tokio::time::Instant::now() + Duration::from_millis(30);
        shell.proc.deadline = Some(deadline);
        let out = tokio::time::timeout(Duration::from_secs(5), shell.run("blocked"))
            .await
            .expect("the deadline did not bound the handler");
        assert_eq!(out.status, 1);
        assert_eq!(
            out.stderr,
            "strands-shell: blocked: execution timeout exceeded\n"
        );
        assert!(started.load(Ordering::SeqCst), "the handler never started");
        assert!(!active.load(Ordering::SeqCst));
        assert_eq!(*seen_deadline.lock().unwrap(), Some(deadline));

        shell.proc.deadline = None;
        assert_eq!(shell.run("echo okay").await.stdout, "okay\n");
    });
}

#[test]
fn dropping_run_cancels_the_handler_future() {
    block_on(async {
        for script in [
            "blocked",
            "{ blocked; } | cat",
            "x=$({ blocked; } | cat)",
            "blocked | cat",
        ] {
            let active = Arc::new(AtomicBool::new(false));
            let started = Arc::new(Notify::new());
            let (flag, start) = (active.clone(), started.clone());
            let mut shell = host_shell(move |_ctx| {
                let (flag, start) = (flag.clone(), start.clone());
                Box::pin(async move {
                    let _active = Active::new(flag);
                    start.notify_one();
                    pending::<()>().await;
                    Ok(Some(0))
                })
            });
            let out = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    _ = shell.run(script) => false,
                    _ = started.notified() => true,
                }
            })
            .await;
            assert_eq!(
                out.ok(),
                Some(true),
                "the handler never started in {script}"
            );
            assert!(active.load(Ordering::SeqCst), "{script}");
            settle().await;
            assert!(
                !active.load(Ordering::SeqCst),
                "dropping run left the handler running in {script}"
            );
        }
    });
}

#[test]
fn captured_stderr_keeps_stage_order() {
    block_on(async {
        let second_wrote = Arc::new(Notify::new());
        let mut shell = host_shell(move |ctx| {
            let second_wrote = second_wrote.clone();
            Box::pin(async move {
                let program = ctx.program().to_string();
                let (_, _, stderr) = ctx.stdio();
                match program.as_str() {
                    "first" => {
                        second_wrote.notified().await;
                        stderr.unwrap().write_all(b"first\n").await?;
                        Ok(Some(0))
                    }
                    "second" => {
                        stderr.unwrap().write_all(b"second\n").await?;
                        second_wrote.notify_one();
                        Ok(Some(0))
                    }
                    _ => Ok(None),
                }
            })
        });
        let out = tokio::time::timeout(Duration::from_secs(5), shell.run("first | second"))
            .await
            .unwrap();
        assert_eq!(out.status, 0);
        assert_eq!(out.stderr, "first\nsecond\n");
    });
}

static PRODUCED: AtomicUsize = AtomicUsize::new(0);
static WAIT_ACTIVE: AtomicBool = AtomicBool::new(false);

struct WaitGuard;

impl Drop for WaitGuard {
    fn drop(&mut self) {
        WAIT_ACTIVE.store(false, Ordering::SeqCst);
    }
}

inventory::submit! {
    CommandEntry {
        name: "fallback_test_producer",
        func: |os, args| Box::pin(producer(os, args)),
    }
}

inventory::submit! {
    CommandEntry {
        name: "fallback_test_noisy_consumer",
        func: |os, args| Box::pin(noisy_consumer(os, args)),
    }
}

inventory::submit! {
    CommandEntry {
        name: "fallback_test_wait",
        func: |_os, _args| Box::pin(async {
            WAIT_ACTIVE.store(true, Ordering::SeqCst);
            let _guard = WaitGuard;
            pending::<()>().await;
            Ok(0)
        }),
    }
}

async fn producer(_os: &dyn Kernel, _args: &[String]) -> CommandResult {
    let mut stdout = strands_shell::io::stdout()?;
    for _ in 0..128 {
        stdout.write_all(b"x").await?;
        PRODUCED.fetch_add(1, Ordering::SeqCst);
    }
    Ok(0)
}

async fn noisy_consumer(_os: &dyn Kernel, _args: &[String]) -> CommandResult {
    let mut stderr = strands_shell::io::stderr()?;
    for _ in 0..128 {
        stderr.write_all(b"e").await?;
    }
    let mut input = Vec::new();
    strands_shell::io::stdin()?.read_to_end(&mut input).await?;
    let mut stdout = strands_shell::io::stdout()?;
    stdout
        .write_all(format!("read {}\n", input.len()).as_bytes())
        .await?;
    Ok(0)
}

#[test]
fn stderr_before_stdin_does_not_deadlock_with_a_handler() {
    block_on(async {
        PRODUCED.store(0, Ordering::SeqCst);
        let mut shell = host_shell(|_ctx| Box::pin(async { Ok(None) }));
        let out = tokio::time::timeout(
            Duration::from_secs(5),
            shell.run("fallback_test_producer | fallback_test_noisy_consumer"),
        )
        .await
        .expect("pipeline deadlocked");
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "read 128\n");
        assert_eq!(out.stderr, "e".repeat(128));
        assert_eq!(PRODUCED.load(Ordering::SeqCst), 128);
    });
}

#[test]
fn without_a_handler_dropping_run_keeps_legacy_task_ownership() {
    block_on(async {
        let mut shell = Shell::builder().build().unwrap();
        let out =
            tokio::time::timeout(Duration::from_millis(20), shell.run("fallback_test_wait")).await;
        assert!(out.is_err());
        settle().await;
        assert!(
            WAIT_ACTIVE.load(Ordering::SeqCst),
            "no-handler cancellation changed"
        );
    });
}
