//! An outside consumer that supplies its own [`Kernel`].
//!
//! `MemKernel` is a small in-memory backend written only against the public
//! API. It does not wrap `VfsKernel`. It implements every required trait
//! method with `&str` paths and leaves the network methods at their defaults.
//! The tests drive it through `Shell::with_kernel` and check the results on
//! the backend itself.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use strands_shell::os::{
    DirEntry, Fd, FdKind, FileStat, HttpRequest, Kernel, OpenFlags, Process, pipe,
};
use strands_shell::parser::{self, CommandLine, Connector, Item, Redirect, WordPart};
use strands_shell::{
    BindInfo, CredInfo, CredKind, FileInfo, FileOpErrorKind, LimitsInfo, Output, Shell, ShellConfig,
};

#[derive(Default)]
struct Tree {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

/// An in-memory backend. Paths under `/ro` refuse writes.
struct MemKernel {
    tree: Arc<Mutex<Tree>>,
}

impl MemKernel {
    fn new() -> Arc<Self> {
        let mut tree = Tree::default();
        for dir in ["/", "/work", "/ro"] {
            tree.dirs.insert(dir.to_string());
        }
        tree.files
            .insert("/work/notes.txt".into(), b"alpha\nbeta\n".to_vec());
        tree.files
            .insert("/ro/fixed.txt".into(), b"fixed\n".to_vec());
        Arc::new(Self {
            tree: Arc::new(Mutex::new(tree)),
        })
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.tree.lock().unwrap().files.get(path).cloned()
    }

    fn has_dir(&self, path: &str) -> bool {
        self.tree.lock().unwrap().dirs.contains(path)
    }
}

fn resolve(proc: &Process, path: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{}", proc.cwd.display(), path)
    };
    let mut parts: Vec<String> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    format!("/{}", parts.join("/"))
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "/",
    }
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no such file or directory")
}

fn refuse_read_only(path: &str) -> io::Result<()> {
    if path == "/ro" || path.starts_with("/ro/") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "read-only backend path",
        ));
    }
    Ok(())
}

#[async_trait]
impl Kernel for MemKernel {
    fn new_process(&self) -> Process {
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), "/work".to_string());
        env.insert("PWD".to_string(), "/work".to_string());
        env.insert("PATH".to_string(), "/bin".to_string());
        env.insert("GREETING".to_string(), "from-kernel".to_string());
        Process::new(PathBuf::from("/work"), env)
    }

    async fn open(&self, proc: &mut Process, path: &str, flags: OpenFlags) -> io::Result<Fd> {
        let abs = resolve(proc, path);
        if flags.write {
            refuse_read_only(&abs)?;
            {
                let mut tree = self.tree.lock().unwrap();
                if !tree.dirs.contains(parent(&abs)) {
                    return Err(not_found());
                }
                if tree.dirs.contains(&abs) {
                    return Err(io::Error::other("is a directory"));
                }
                let entry = tree.files.entry(abs.clone()).or_default();
                if flags.truncate && !flags.append {
                    entry.clear();
                }
            }
            let (tx, mut rx) = pipe(64);
            let fd = proc.alloc_fd(FdKind::ChannelWriter {
                tx,
                error_flag: None,
            })?;
            let tree = self.tree.clone();
            tokio::task::spawn_local(async move {
                while let Some(chunk) = rx.recv().await {
                    if let Some(file) = tree.lock().unwrap().files.get_mut(&abs) {
                        file.extend_from_slice(&chunk);
                    }
                }
            });
            return Ok(fd);
        }
        let data = self.file(&abs).ok_or_else(not_found)?;
        let (tx, rx) = pipe(data.len().max(1));
        let _ = tx.send(Bytes::from(data)).await;
        drop(tx);
        proc.alloc_fd(FdKind::ChannelReader {
            rx,
            buf: Vec::new(),
        })
    }

    async fn list_dir(&self, proc: &Process, path: &str) -> io::Result<Vec<DirEntry>> {
        let abs = resolve(proc, path);
        let tree = self.tree.lock().unwrap();
        if !tree.dirs.contains(&abs) {
            return Err(not_found());
        }
        let mut out: Vec<DirEntry> = Vec::new();
        for dir in tree.dirs.iter().filter(|d| *d != "/" && parent(d) == abs) {
            out.push(DirEntry {
                name: dir.rsplit('/').next().unwrap().to_string(),
                is_dir: true,
            });
        }
        for file in tree.files.keys().filter(|f| parent(f) == abs) {
            out.push(DirEntry {
                name: file.rsplit('/').next().unwrap().to_string(),
                is_dir: false,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn change_dir(&self, proc: &mut Process, path: &str) -> io::Result<()> {
        let abs = resolve(proc, path);
        if !self.has_dir(&abs) {
            return Err(not_found());
        }
        proc.cwd = PathBuf::from(abs);
        Ok(())
    }

    async fn stat(&self, proc: &Process, path: &str) -> FileStat {
        let abs = resolve(proc, path);
        let tree = self.tree.lock().unwrap();
        if let Some(data) = tree.files.get(&abs) {
            FileStat {
                exists: true,
                is_file: true,
                len: data.len() as u64,
                mode: 0o100644,
                ..FileStat::default()
            }
        } else if tree.dirs.contains(&abs) {
            FileStat {
                exists: true,
                is_dir: true,
                mode: 0o040755,
                ..FileStat::default()
            }
        } else {
            FileStat::default()
        }
    }

    async fn lstat(&self, proc: &Process, path: &str) -> FileStat {
        self.stat(proc, path).await
    }

    async fn access(&self, proc: &Process, path: &str, _mode: i32) -> bool {
        self.stat(proc, path).await.exists
    }

    async fn canonicalize(&self, proc: &Process, path: &str) -> io::Result<PathBuf> {
        let abs = resolve(proc, path);
        if self.stat(proc, &abs).await.exists {
            Ok(PathBuf::from(abs))
        } else {
            Err(not_found())
        }
    }

    async fn is_executable(&self, _proc: &Process, _path: &str) -> bool {
        false
    }

    async fn glob(&self, _proc: &Process, _pattern: &str) -> Vec<String> {
        Vec::new()
    }

    fn isatty(&self, _fd: i32) -> bool {
        false
    }

    async fn remove_file(&self, proc: &Process, path: &str) -> io::Result<()> {
        let abs = resolve(proc, path);
        refuse_read_only(&abs)?;
        let mut tree = self.tree.lock().unwrap();
        tree.files.remove(&abs).map(|_| ()).ok_or_else(not_found)
    }

    async fn remove_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        let abs = resolve(proc, path);
        refuse_read_only(&abs)?;
        let mut tree = self.tree.lock().unwrap();
        if tree.dirs.remove(&abs) {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    async fn create_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        let abs = resolve(proc, path);
        refuse_read_only(&abs)?;
        let mut tree = self.tree.lock().unwrap();
        if !tree.dirs.contains(parent(&abs)) {
            return Err(not_found());
        }
        if tree.dirs.contains(&abs) || tree.files.contains_key(&abs) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "file exists"));
        }
        tree.dirs.insert(abs);
        Ok(())
    }

    async fn rename(&self, proc: &Process, from: &str, to: &str) -> io::Result<()> {
        let (from, to) = (resolve(proc, from), resolve(proc, to));
        refuse_read_only(&from)?;
        refuse_read_only(&to)?;
        let mut tree = self.tree.lock().unwrap();
        let data = tree.files.remove(&from).ok_or_else(not_found)?;
        tree.files.insert(to, data);
        Ok(())
    }

    async fn symlink(&self, _proc: &Process, _target: &str, _link: &str) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no symlinks"))
    }

    async fn read_link(&self, _proc: &Process, _path: &str) -> io::Result<String> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no symlinks"))
    }

    async fn set_permissions(&self, _proc: &Process, _path: &str, _mode: u32) -> io::Result<()> {
        Ok(())
    }

    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&rt, future)
}

/// Yield until the backend's write task commits `path`, or give up.
async fn committed(kernel: &MemKernel, path: &str, expected: &[u8]) -> Option<Vec<u8>> {
    for _ in 0..1024 {
        if kernel.file(path).as_deref() == Some(expected) {
            break;
        }
        tokio::task::yield_now().await;
    }
    kernel.file(path)
}

fn shell_over(kernel: &Arc<MemKernel>) -> Shell {
    let dyn_kernel: Arc<dyn Kernel> = kernel.clone();
    Shell::with_kernel(dyn_kernel)
}

#[test]
fn shell_starts_with_the_kernel_process() {
    let kernel = MemKernel::new();
    let mut shell = shell_over(&kernel);
    assert_eq!(shell.get_env("GREETING"), Some("from-kernel"));
    block_on(async {
        let out: Output = shell.run("pwd; echo \"$GREETING\" \"$HOME\"").await;
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "/work\nfrom-kernel /work\n");
        assert_eq!(out.stderr, "");
    });
}

#[test]
fn commands_read_backend_bytes() {
    let kernel = MemKernel::new();
    let mut shell = shell_over(&kernel);
    block_on(async {
        let out = shell.run("cat notes.txt | wc -l").await;
        assert_eq!(
            (out.status, out.stdout.trim(), out.stderr.as_str()),
            (0, "2", "")
        );

        let out = shell.run("grep beta /work/notes.txt").await;
        assert_eq!((out.status, out.stdout.as_str()), (0, "beta\n"));

        let out = shell.run("cat /ro/fixed.txt").await;
        assert_eq!((out.status, out.stdout.as_str()), (0, "fixed\n"));
    });
}

#[test]
fn redirects_and_directory_changes_reach_the_backend() {
    let kernel = MemKernel::new();
    let mut shell = shell_over(&kernel);
    block_on(async {
        let out = shell.run("echo one > out.txt; echo two >> out.txt").await;
        assert_eq!((out.status, out.stdout.as_str()), (0, ""));
        assert_eq!(
            committed(&kernel, "/work/out.txt", b"one\ntwo\n").await,
            Some(b"one\ntwo\n".to_vec())
        );

        let out = shell.run("mkdir sub && cd sub && pwd").await;
        assert_eq!((out.status, out.stdout.as_str()), (0, "/work/sub\n"));
        assert!(kernel.has_dir("/work/sub"));

        let out = shell
            .run("mv ../out.txt moved.txt && rm ../notes.txt")
            .await;
        assert_eq!(out.status, 0, "stderr: {}", out.stderr);
        assert_eq!(kernel.file("/work/out.txt"), None);
        assert_eq!(kernel.file("/work/notes.txt"), None);
        assert_eq!(
            kernel.file("/work/sub/moved.txt"),
            Some(b"one\ntwo\n".to_vec())
        );
    });
}

#[test]
fn failed_redirects_return_status_one_and_leave_the_backend_unchanged() {
    let kernel = MemKernel::new();
    let mut shell = shell_over(&kernel);
    block_on(async {
        let out = shell.run("echo hi > /missing/x").await;
        assert_eq!((out.status, out.stdout.as_str()), (1, ""));
        assert_eq!(kernel.file("/missing/x"), None);

        let out = shell.run("echo hi > /ro/fixed.txt").await;
        assert_eq!((out.status, out.stdout.as_str()), (1, ""));
        assert_eq!(kernel.file("/ro/fixed.txt"), Some(b"fixed\n".to_vec()));
    });
}

#[test]
fn file_api_uses_the_backend_and_propagates_its_errors() {
    let kernel = MemKernel::new();
    let mut shell = shell_over(&kernel);
    block_on(async {
        let bytes: Vec<u8> = shell.read_file("/work/notes.txt").await.unwrap();
        assert_eq!(bytes, b"alpha\nbeta\n");

        shell
            .write_file("/work/new.bin", b"\x00\xffdata")
            .await
            .unwrap();
        assert_eq!(kernel.file("/work/new.bin"), Some(b"\x00\xffdata".to_vec()));

        let listing: Vec<FileInfo> = shell.list_files("/work").await.unwrap();
        let listing: Vec<(String, Option<bool>, Option<u64>)> = listing
            .into_iter()
            .map(|f| (f.name, f.is_dir, f.size))
            .collect();
        assert_eq!(
            listing,
            vec![
                ("new.bin".to_string(), Some(false), Some(6)),
                ("notes.txt".to_string(), Some(false), Some(11)),
            ]
        );

        shell.remove_file("/work/new.bin").await.unwrap();
        assert_eq!(kernel.file("/work/new.bin"), None);

        let err = shell.read_file("/work/absent").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(FileOpErrorKind::classify(&err), FileOpErrorKind::NotFound);
        assert_eq!(err.to_string(), "/work/absent: no such file or directory");

        let err = shell.write_file("/ro/fixed.txt", b"x").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            FileOpErrorKind::classify(&err),
            FileOpErrorKind::PermissionDenied
        );
        assert_eq!(err.to_string(), "/ro/fixed.txt: read-only backend path");
        assert_eq!(kernel.file("/ro/fixed.txt"), Some(b"fixed\n".to_vec()));

        let err = shell.remove_file("/ro/fixed.txt").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        let err = shell.list_files("/absent").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    });
}

#[test]
fn kernel_accessor_returns_the_supplied_trait_object() {
    let kernel = MemKernel::new();
    let shell = shell_over(&kernel);
    let accessed: &Arc<dyn Kernel> = shell.kernel();
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(accessed),
        Arc::as_ptr(&kernel)
    ));

    let proc = accessed.new_process();
    assert_eq!(proc.cwd, PathBuf::from("/work"));
    assert_eq!(proc.get_env("GREETING"), Some("from-kernel"));
    assert_eq!(accessed.now(), kernel.now());
}

#[test]
fn network_methods_keep_their_defaults() {
    let kernel = MemKernel::new();
    let mut shell = shell_over(&kernel);
    let accessed = shell.kernel().clone();
    assert!(accessed.check_url("https://example.com/").is_ok());
    assert!(
        accessed
            .resolve_credential("https://example.com/", "GET")
            .is_empty()
    );
    block_on(async {
        let request = HttpRequest {
            method: "GET".into(),
            url: "https://example.com/".into(),
            headers: Vec::new(),
            body: None,
            insecure: false,
            max_response: 0,
        };
        let err = accessed.http_request(request).await.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);

        let out = shell.run("curl -s https://example.com/").await;
        assert_eq!((out.status, out.stdout.as_str()), (6, ""));
    });
}

#[test]
fn config_snapshot_types_stay_public() {
    let shell = shell_over(&MemKernel::new());
    let cfg: &ShellConfig = shell.config();
    let binds: &Vec<BindInfo> = &cfg.binds;
    let credentials: &Vec<CredInfo> = &cfg.credentials;
    let limits: &LimitsInfo = &cfg.limits;
    assert!(binds.is_empty());
    assert!(credentials.is_empty());
    assert!(cfg.allowed_urls.is_empty());
    assert!(cfg.env.is_empty());
    assert_eq!(cfg.umask, 0o022);
    assert_eq!(cfg.timeout_secs, None);
    assert_eq!(limits.max_depth, LimitsInfo::default().max_depth);

    let built = Shell::builder()
        .credential("https://api.example.com/*", CredKind::Bearer, "secret")
        .build()
        .unwrap();
    let cred: &CredInfo = &built.config().credentials[0];
    let url: &String = &cred.url;
    let kind: &'static str = cred.kind;
    let methods: &Vec<String> = &cred.methods;
    let param: &Option<String> = &cred.param;
    let env_var: &Option<String> = &cred.env_var;
    let from_literal: bool = cred.from_literal;
    assert_eq!(url, "https://api.example.com/*");
    assert_eq!(kind, CredKind::Bearer.as_str());
    assert!(methods.is_empty());
    assert_eq!((param, env_var, from_literal), (&None, &None, true));

    let host = std::env::temp_dir().to_string_lossy().into_owned();
    let built = Shell::builder()
        .bind_direct_readonly(&host, "/work")
        .build()
        .unwrap();
    let bind: &BindInfo = &built.config().binds[0];
    let source: &String = &bind.source;
    let destination: &String = &bind.destination;
    let mode: &'static str = bind.mode;
    let readonly: bool = bind.readonly;
    assert_eq!((source, destination.as_str()), (&host, "/work"));
    assert_eq!((mode, readonly), ("direct", true));
}

#[test]
fn parser_results_stay_public() {
    let line: CommandLine = parser::parse("echo hi > out.txt && cat out.txt").unwrap();
    assert_eq!(line.len(), 2);

    let (Item::Pipeline(first, false), Some(Connector::And)) = &line[0] else {
        panic!("unexpected first item: {:?}", line[0]);
    };
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].args,
        vec![
            vec![WordPart::Literal("echo".into())],
            vec![WordPart::Literal("hi".into())],
        ]
    );
    assert_eq!(
        first[0].redirects,
        vec![Redirect::Write(
            1,
            vec![WordPart::Literal("out.txt".into())]
        )]
    );

    let (Item::Pipeline(second, false), None) = &line[1] else {
        panic!("unexpected second item: {:?}", line[1]);
    };
    assert_eq!(second[0].args[0], vec![WordPart::Literal("cat".into())]);

    let err: String = parser::parse("echo 'unterminated").unwrap_err();
    assert!(!err.is_empty());
}
