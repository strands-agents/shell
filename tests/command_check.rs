//! A custom kernel that decides each command Shell runs itself.
//!
//! `CheckingKernel` forwards every operation to the default kernel, records
//! each `check_command` and `command_finished` call, and refuses `rm`.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use strands_shell::Shell;
use strands_shell::os::{
    DirEntry, Fd, FileStat, HttpRequest, HttpResponse, Kernel, OpenFlags, Process,
};

#[derive(Clone, Debug, PartialEq)]
enum Event {
    Check(String, Vec<String>, PathBuf),
    Finished(String, Vec<String>, PathBuf, i32),
}

struct CheckingKernel {
    inner: Arc<dyn Kernel>,
    events: Arc<Mutex<Vec<Event>>>,
}

#[async_trait]
impl Kernel for CheckingKernel {
    async fn check_command(&self, program: &str, args: &[String], cwd: &Path) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::Check(
            program.to_string(),
            args.to_vec(),
            cwd.to_path_buf(),
        ));
        if program == "rm" {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "denied by policy",
            ));
        }
        Ok(())
    }

    fn command_finished(&self, program: &str, args: &[String], cwd: &Path, status: i32) {
        self.events.lock().unwrap().push(Event::Finished(
            program.to_string(),
            args.to_vec(),
            cwd.to_path_buf(),
            status,
        ));
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

fn checking_shell() -> (Shell, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let inner = Shell::builder().build().unwrap().kernel().clone();
    let shell = Shell::with_kernel(Arc::new(CheckingKernel {
        inner,
        events: events.clone(),
    }));
    (shell, events)
}

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
            Event::Finished(..) => None,
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
fn commands_for_the_fallback_are_not_checked() {
    block_on(async {
        let (mut shell, events) = checking_shell();
        let out = shell.run("missing arg").await;
        assert_eq!(out.status, 127);
        assert!(events.lock().unwrap().is_empty());
    });
}
