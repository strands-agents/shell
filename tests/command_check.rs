//! A custom kernel that decides each command before it runs.
//!
//! `CheckingKernel` forwards every operation to the default kernel, records
//! each `check_command`, `command_finished`, and `command_dropped` call, and
//! refuses the programs it is given. `FallbackKernel` also has a command
//! fallback and records each `check_fallback` call.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use strands_shell::Shell;
use strands_shell::os::{
    CommandContext, CommandFallback, CommandRequest, DirEntry, Fd, FileStat, HttpRequest,
    HttpResponse, Kernel, OpenFlags, Process,
};

/// Implement `Kernel` for a type with an `inner` kernel: the given methods,
/// and every other required method forwarded to `inner`.
macro_rules! forwarding_kernel {
    ($kernel:ty { $($own:tt)* }) => {
        #[async_trait]
        impl Kernel for $kernel {
            $($own)*

            fn new_process(&self) -> Process {
                self.inner.new_process()
            }

            async fn open(
                &self,
                proc: &mut Process,
                path: &str,
                flags: OpenFlags,
            ) -> io::Result<Fd> {
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

            async fn set_permissions(
                &self,
                proc: &Process,
                path: &str,
                mode: u32,
            ) -> io::Result<()> {
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
    };
}

#[derive(Clone, Debug, PartialEq)]
enum Event {
    Check(String, Vec<String>, PathBuf),
    Finished(String, Vec<String>, PathBuf, i32),
    Dropped(String, Vec<String>, PathBuf),
    CheckFallback(String, Vec<String>, PathBuf),
}

struct CheckingKernel {
    inner: Arc<dyn Kernel>,
    events: Arc<Mutex<Vec<Event>>>,
    denied: &'static [&'static str],
    fail_record: bool,
}

forwarding_kernel!(CheckingKernel {
    async fn check_command(&self, request: &CommandRequest) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::Check(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
        ));
        if self.denied.contains(&request.program()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "denied by policy",
            ));
        }
        Ok(())
    }

    async fn command_finished(&self, request: &CommandRequest, status: i32) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::Finished(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
            status,
        ));
        if self.fail_record {
            return Err(io::Error::other("store unavailable"));
        }
        Ok(())
    }

    fn command_dropped(&self, request: &CommandRequest) {
        self.events.lock().unwrap().push(Event::Dropped(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
        ));
    }
});

/// A kernel that checks and records commands like `CheckingKernel`, but
/// keeps the default `command_dropped`.
struct PlainKernel {
    inner: Arc<dyn Kernel>,
    events: Arc<Mutex<Vec<Event>>>,
}

forwarding_kernel!(PlainKernel {
    async fn check_command(&self, request: &CommandRequest) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::Check(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
        ));
        if request.program() == "rm" {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "denied by policy",
            ));
        }
        Ok(())
    }

    async fn command_finished(&self, request: &CommandRequest, status: i32) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::Finished(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
            status,
        ));
        Ok(())
    }
});

/// A kernel with a command fallback that records each program it runs,
/// and refuses the fallback programs it is given.
struct FallbackKernel {
    inner: Arc<dyn Kernel>,
    events: Arc<Mutex<Vec<Event>>>,
    ran: Arc<Mutex<Vec<String>>>,
    has_fallback: bool,
    denied: &'static [&'static str],
}

#[async_trait]
impl CommandFallback for FallbackKernel {
    async fn run(&self, ctx: &mut CommandContext) -> io::Result<Option<i32>> {
        self.ran.lock().unwrap().push(ctx.program().to_string());
        Ok(Some(0))
    }
}

forwarding_kernel!(FallbackKernel {
    fn command_fallback(&self) -> Option<&dyn CommandFallback> {
        self.has_fallback.then_some(self as &dyn CommandFallback)
    }

    async fn check_command(&self, request: &CommandRequest) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::Check(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
        ));
        Ok(())
    }

    async fn check_fallback(&self, request: &CommandRequest) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::CheckFallback(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
        ));
        if self.denied.contains(&request.program()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "denied by policy",
            ));
        }
        Ok(())
    }

    fn command_dropped(&self, request: &CommandRequest) {
        self.events.lock().unwrap().push(Event::Dropped(
            request.program().to_string(),
            request.args().to_vec(),
            request.cwd().to_path_buf(),
        ));
    }
});

/// A kernel with a command fallback that keeps the default `check_fallback`.
struct DefaultFallbackKernel {
    inner: Arc<dyn Kernel>,
    ran: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl CommandFallback for DefaultFallbackKernel {
    async fn run(&self, ctx: &mut CommandContext) -> io::Result<Option<i32>> {
        self.ran.lock().unwrap().push(ctx.program().to_string());
        Ok(Some(0))
    }
}

forwarding_kernel!(DefaultFallbackKernel {
    fn command_fallback(&self) -> Option<&dyn CommandFallback> {
        Some(self)
    }
});

type FallbackShell = (Shell, Arc<Mutex<Vec<Event>>>, Arc<Mutex<Vec<String>>>);

fn fallback_shell(has_fallback: bool, denied: &'static [&'static str]) -> FallbackShell {
    let events = Arc::new(Mutex::new(Vec::new()));
    let ran = Arc::new(Mutex::new(Vec::new()));
    let inner = Shell::builder().build().unwrap().kernel().clone();
    let shell = Shell::with_kernel(Arc::new(FallbackKernel {
        inner,
        events: events.clone(),
        ran: ran.clone(),
        has_fallback,
        denied,
    }));
    (shell, events, ran)
}

fn checking_shell() -> (Shell, Arc<Mutex<Vec<Event>>>) {
    shell_with(&["rm"], false)
}

fn shell_with(
    denied: &'static [&'static str],
    fail_record: bool,
) -> (Shell, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let inner = Shell::builder().build().unwrap().kernel().clone();
    let shell = Shell::with_kernel(Arc::new(CheckingKernel {
        inner,
        events: events.clone(),
        denied,
        fail_record,
    }));
    (shell, events)
}

fn finished(events: &Mutex<Vec<Event>>) -> Vec<Event> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, Event::Finished(..)))
        .cloned()
        .collect()
}

const STDERR_MISSING_DIR: &str =
    "strands-shell: /missing/dir/f: no such file or directory: /missing/dir\n";

fn block_on<F: Future>(future: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&rt, future)
}

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn checked_programs(events: &Mutex<Vec<Event>>) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Event::Check(program, ..) => Some(program.clone()),
            Event::Finished(..) | Event::Dropped(..) | Event::CheckFallback(..) => None,
        })
        .collect()
}

#[test]
fn an_allowed_command_runs_and_reports_its_status() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("mkdir -p /tmp/work && cd /tmp/work").await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        events.lock().unwrap().clear();

        let out = shell.run("X=hello; echo $X 'a b'; false").await;
        assert_eq!(out.stdout, "hello a b\n");
        assert_eq!(out.status, 1);
        let cwd = PathBuf::from("/tmp/work");
        assert_eq!(
            *events.lock().unwrap(),
            [
                Event::Check("echo".into(), strings(&["hello", "a b"]), cwd.clone()),
                Event::Finished("echo".into(), strings(&["hello", "a b"]), cwd.clone(), 0),
                Event::Check("false".into(), vec![], cwd.clone()),
                Event::Finished("false".into(), vec![], cwd, 1),
            ]
        );
    });
}

#[test]
fn a_denied_command_does_not_run_and_reports_the_denial() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        shell.run("echo data > /tmp/keep").await;
        let out = shell.run("rm /tmp/keep").await;
        assert_eq!(out.status, 126);
        assert_eq!(out.stderr, "strands-shell: rm: denied by policy\n");
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, Event::Finished(program, ..) if program == "rm"))
        );
        assert_eq!(shell.run("cat /tmp/keep").await.stdout, "data\n");
    });
}

#[test]
fn a_denied_stage_stops_the_whole_pipeline() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("touch /tmp/first | rm /tmp/other").await;
        assert_eq!(out.status, 126);
        assert_eq!(checked_programs(&events), ["touch", "rm"]);
        assert_eq!(shell.run("test -e /tmp/first").await.status, 1);
    });
}

#[test]
fn nested_commands_are_checked() {
    block_on(async {
        for script in [
            "eval 'rm /tmp/x'",
            "f() { rm /tmp/x; }; f",
            "echo /tmp/x | xargs rm",
            "touch /tmp/x; find /tmp/x -exec rm {} \\;",
            "x=$(rm /tmp/x)",
            "echo 'rm /tmp/x' > /tmp/s; . /tmp/s",
            "echo 'rm /tmp/x' > /tmp/s; sh /tmp/s",
            "lua -e 'os.execute(\"rm /tmp/x\")'",
            "trap 'rm /tmp/x' EXIT",
        ] {
            let (mut shell, events) = checking_shell();
            shell.run(script).await;
            let checks = events.lock().unwrap().clone();
            assert!(
                checks.contains(&Event::Check(
                    "rm".into(),
                    strings(&["/tmp/x"]),
                    PathBuf::from("/home/lash")
                )),
                "{script}: {checks:?}"
            );
        }
    });
}

#[test]
fn unknown_commands_without_a_fallback_are_not_checked() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("missing arg").await;
        assert_eq!(out.status, 127);
        assert!(events.lock().unwrap().is_empty());
    });
}

#[test]
fn builtin_is_unwrapped_before_the_check() {
    block_on(async {
        for script in [
            "builtin echo hi",
            "builtin builtin echo hi",
            "true | builtin echo hi",
            "builtin echo hi | cat",
            "X=builtin; $X echo hi",
            "\"builtin\" echo hi",
            "eval 'true | builtin echo hi'",
            "command builtin echo hi",
        ] {
            let (mut shell, events) = shell_with(&["echo"], false);
            let out = shell.run(script).await;
            assert_eq!(out.status, 126, "{script}");
            assert_eq!(out.stdout, "", "{script}");
            assert_eq!(
                checked_programs(&events).last().map(String::as_str),
                Some("echo"),
                "{script}"
            );
            assert!(finished(&events).is_empty(), "{script}: {events:?}");
        }
    });
}

#[test]
fn builtin_find_in_a_later_stage_is_checked_as_find() {
    block_on(async {
        let (mut shell, events) = shell_with(&["find"], false);
        shell.run("mkdir -p /tmp/d; touch /tmp/d/f").await;
        events.lock().unwrap().clear();
        let out = shell
            .run("true | builtin find /tmp/d -name f -exec rm {} ';'")
            .await;
        assert_eq!(out.status, 126);
        assert_eq!(checked_programs(&events), ["true", "find"]);
        assert!(finished(&events).is_empty(), "{events:?}");
        assert_eq!(shell.run("test -e /tmp/d/f").await.status, 0);
    });
}

#[test]
fn an_allowed_builtin_reports_the_name_that_was_checked() {
    block_on(async {
        let (mut shell, events) = shell_with(&["echo"], false);
        let out = shell.run("builtin builtin printf ok").await;
        assert_eq!(out.stdout, "ok");
        let cwd = PathBuf::from("/home/lash");
        assert_eq!(
            *events.lock().unwrap(),
            [
                Event::Check("printf".into(), strings(&["ok"]), cwd.clone()),
                Event::Finished("printf".into(), strings(&["ok"]), cwd, 0),
            ]
        );
    });
}

#[test]
fn builtin_with_no_builtin_name_is_checked_as_builtin() {
    block_on(async {
        let (mut shell, events) = shell_with(&["cat"], false);
        let out = shell.run("builtin cat /dev/null").await;
        assert_eq!(out.status, 1);
        assert_eq!(checked_programs(&events), ["builtin"]);
    });
}

#[test]
fn each_pipeline_stage_reports_its_own_status() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("true | false | echo x").await;
        assert_eq!(out.status, 0);
        let cwd = PathBuf::from("/home/lash");
        assert_eq!(
            finished(&events),
            [
                Event::Finished("true".into(), vec![], cwd.clone(), 0),
                Event::Finished("false".into(), vec![], cwd.clone(), 1),
                Event::Finished("echo".into(), strings(&["x"]), cwd, 0),
            ]
        );
    });
}

#[test]
fn functions_and_scripts_report_their_status() {
    block_on(async {
        let cwd = PathBuf::from("/home/lash");
        let (mut shell, events) = checking_shell();
        shell.run("f() { return 3; }; f a").await;
        assert!(
            finished(&events).contains(&Event::Finished(
                "f".into(),
                strings(&["a"]),
                cwd.clone(),
                3
            )),
            "{events:?}"
        );

        let (mut shell, events) = checking_shell();
        shell.run("echo 'exit 4' > /tmp/s; sh /tmp/s").await;
        assert!(
            finished(&events).contains(&Event::Finished("sh".into(), strings(&["/tmp/s"]), cwd, 4)),
            "{events:?}"
        );
    });
}

fn check(program: &str, args: &[&str]) -> Event {
    Event::Check(program.into(), strings(args), PathBuf::from("/home/lash"))
}

fn dropped(program: &str, args: &[&str]) -> Event {
    Event::Dropped(program.into(), strings(args), PathBuf::from("/home/lash"))
}

#[test]
fn an_allowed_command_whose_redirect_fails_is_reported_as_dropped() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("echo hi > /missing/dir/f").await;
        assert_eq!(out.status, 1);
        assert_eq!(
            *events.lock().unwrap(),
            [check("echo", &["hi"]), dropped("echo", &["hi"])]
        );

        let (mut shell, events) = checking_shell();
        let out = shell
            .run("f() { touch /tmp/ran; }; f > /missing/dir/f; test -e /tmp/ran")
            .await;
        assert_eq!(out.status, 1);
        assert_eq!(
            *events.lock().unwrap(),
            [
                check("f", &[]),
                dropped("f", &[]),
                check("test", &["-e", "/tmp/ran"]),
                Event::Finished(
                    "test".into(),
                    strings(&["-e", "/tmp/ran"]),
                    PathBuf::from("/home/lash"),
                    1,
                ),
            ]
        );

        let (mut shell, events) = checking_shell();
        let out = shell.run("true > /missing/dir/f | echo b").await;
        assert_eq!(out.status, 1);
        assert_eq!(
            *events.lock().unwrap(),
            [
                check("true", &[]),
                check("echo", &["b"]),
                dropped("true", &[]),
                dropped("echo", &["b"]),
            ]
        );
    });
}

#[test]
fn allowed_stages_before_a_refused_stage_are_reported_as_dropped() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("echo a | true | rm x").await;
        assert_eq!(out.status, 126);
        assert_eq!(
            *events.lock().unwrap(),
            [
                check("echo", &["a"]),
                check("true", &[]),
                check("rm", &["x"]),
                dropped("echo", &["a"]),
                dropped("true", &[]),
            ]
        );
    });
}

#[test]
fn a_cancelled_pipeline_reports_its_unfinished_stages_as_dropped() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let run = shell.run("sleep 5 | cat");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), run)
                .await
                .is_err()
        );
        assert_eq!(
            *events.lock().unwrap(),
            [
                check("sleep", &["5"]),
                check("cat", &[]),
                dropped("sleep", &["5"]),
                dropped("cat", &[]),
            ]
        );
    });
}

#[test]
fn a_command_that_finished_is_not_reported_as_dropped() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        shell
            .run("echo a | false; f() { return 2; }; f; builtin printf x")
            .await;
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, Event::Dropped(..))),
            "{events:?}"
        );
    });
}

#[test]
fn a_kernel_without_command_dropped_sees_no_change() {
    block_on(async {
        for (script, status, stderr, expected) in [
            (
                "echo hi > /missing/dir/f",
                1,
                STDERR_MISSING_DIR,
                vec![check("echo", &["hi"])],
            ),
            (
                "true > /missing/dir/f | echo b",
                1,
                STDERR_MISSING_DIR,
                vec![check("true", &[]), check("echo", &["b"])],
            ),
            (
                "echo a | rm x",
                126,
                "strands-shell: rm: denied by policy\n",
                vec![check("echo", &["a"]), check("rm", &["x"])],
            ),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let inner = Shell::builder().build().unwrap().kernel().clone();
            let mut shell = Shell::with_kernel(Arc::new(PlainKernel {
                inner,
                events: events.clone(),
            }));
            let out = shell.run(script).await;
            assert_eq!(out.status, status, "{script}");
            assert_eq!(out.stdout, "", "{script}");
            assert_eq!(out.stderr, stderr, "{script}");
            assert_eq!(*events.lock().unwrap(), expected, "{script}");
        }
    });
}

#[test]
fn a_failed_record_is_reported_and_changes_the_status() {
    block_on(async {
        let (mut shell, events) = shell_with(&[], true);
        let out = shell.run("echo hi").await;
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.status, 125);
        assert_eq!(
            out.stderr,
            "strands-shell: echo: finished with status 0, but the kernel could not record it: store unavailable\n"
        );
        assert_eq!(finished(&events).len(), 1);
    });
}

fn check_fallback(program: &str, args: &[&str]) -> Event {
    Event::CheckFallback(program.into(), strings(args), PathBuf::from("/home/lash"))
}

#[test]
fn a_refused_fallback_stage_stops_every_stage() {
    block_on(async {
        let (mut shell, _, ran) = fallback_shell(true, &["tool"]);
        let out = shell.run("mkdir /tmp/made | tool a").await;
        assert_eq!(out.status, 126);
        assert_eq!(out.stderr, "strands-shell: tool: denied by policy\n");
        assert!(ran.lock().unwrap().is_empty());
        assert_eq!(shell.run("test -e /tmp/made").await.status, 1);
    });
}

#[test]
fn allowed_stages_before_a_refused_fallback_stage_are_reported_as_dropped() {
    block_on(async {
        let (mut shell, events, _) = fallback_shell(true, &["tool"]);
        let out = shell.run("echo a | other b | tool c").await;
        assert_eq!(out.status, 126);
        assert_eq!(
            *events.lock().unwrap(),
            [
                check("echo", &["a"]),
                check_fallback("other", &["b"]),
                check_fallback("tool", &["c"]),
                dropped("echo", &["a"]),
            ]
        );
    });
}

#[test]
fn an_allowed_fallback_stage_is_asked_before_any_stage_runs() {
    block_on(async {
        let (mut shell, events, ran) = fallback_shell(true, &[]);
        let out = shell.run("tool a | true").await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(
            events.lock().unwrap()[..2],
            [check_fallback("tool", &["a"]), check("true", &[])]
        );
        assert_eq!(*ran.lock().unwrap(), ["tool"]);
    });
}

#[test]
fn a_command_shell_runs_itself_is_not_sent_to_check_fallback() {
    block_on(async {
        let (mut shell, events, ran) = fallback_shell(true, &["tool"]);
        let out = shell.run("tool() { true; }; tool a").await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, Event::CheckFallback(..))),
            "{events:?}"
        );
        assert!(ran.lock().unwrap().is_empty());
    });
}

#[test]
fn a_kernel_without_a_fallback_is_not_asked_about_fallback_stages() {
    block_on(async {
        let (mut shell, events, _) = fallback_shell(false, &["tool"]);
        let out = shell.run("tool a").await;
        assert_eq!(out.status, 127);
        assert!(events.lock().unwrap().is_empty());
    });
}

#[test]
fn a_kernel_without_check_fallback_runs_fallback_stages_as_before() {
    block_on(async {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let inner = Shell::builder().build().unwrap().kernel().clone();
        let mut shell = Shell::with_kernel(Arc::new(DefaultFallbackKernel {
            inner,
            ran: ran.clone(),
        }));
        let out = shell.run("mkdir /tmp/made | tool a").await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(out.stderr, "");
        assert_eq!(*ran.lock().unwrap(), ["tool"]);
        assert_eq!(shell.run("test -d /tmp/made").await.status, 0);
    });
}
