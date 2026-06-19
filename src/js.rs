//! Node.js bindings for Strands Shell shell, via napi-rs.
//!
//! Mirrors `src/python.rs` in shape and behavior. The differences are
//! language-idiomatic, not semantic:
//!
//! * All I/O methods (`run`, `readFile`, `writeFile`, `removeFile`,
//!   `listFiles`, `build`) return Promises.
//! * Names are camelCase (auto from `napi-derive`).
//! * Bytes use `Uint8Array` for forward-compat with the future browser
//!   binding (`Buffer` is a `Uint8Array` so Node users pass it directly).
//!
//! Threading model
//! ---------------
//! `crate::Shell` is `!Send` (it holds `Rc<Vec<NamedMcpClient>>`), so we
//! cannot share it across napi's blocking thread pool. Instead, each
//! `Shell` instance owns a **dedicated worker thread** that holds the
//! `crate::Shell` + its current-thread tokio runtime. Each napi async
//! method dispatches a closure to that thread over an mpsc channel and
//! awaits the result via a oneshot. Concurrent calls on the same
//! `Shell` are serialized in FIFO order — matching the doc's edge case.

use std::sync::Mutex;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi::tokio::sync::oneshot;
use napi_derive::napi;

use crate::shell::FileOpErrorKind;

/// Build a napi `Error` for a file-op failure, encoding the classification so
/// the JS wrapper (`index.js`) can re-throw it as a typed `ShellError`
/// subclass. The reason is `"{code}\t{path}\t{message}"`; the wrapper splits on
/// the first two tabs. We can't attach custom JS properties from Rust through
/// napi, so this tab-delimited envelope is the channel. `\t` is safe: VFS paths
/// don't contain tabs, and splitting with a limit keeps tabs inside `message`.
fn file_error(path: &str, err: &std::io::Error) -> Error {
    let code = match FileOpErrorKind::classify(err) {
        FileOpErrorKind::NotFound => "ENOENT",
        FileOpErrorKind::PermissionDenied => "EACCES",
        FileOpErrorKind::TooLarge => "EFBIG",
        FileOpErrorKind::Other => "EOTHER",
    };
    Error::from_reason(format!("{code}\t{path}\t{err}"))
}

// ---------------------------------------------------------------------------
// Worker thread plumbing
// ---------------------------------------------------------------------------

/// A unit of work the worker thread runs against the inner shell. The
/// closure is boxed so each napi method can capture its own arguments
/// without leaking concrete types into this signature.
type Job = Box<dyn FnOnce(&mut crate::Shell, &tokio::runtime::Runtime) + Send + 'static>;

struct Worker {
    tx: std_mpsc::Sender<Job>,
}

impl Worker {
    /// Spawn a dedicated thread that builds `crate::Shell` from a
    /// `ShellBuilder` *on the new thread* and then owns it for the
    /// rest of its life.
    ///
    /// We can't move a constructed `crate::Shell` across threads
    /// because it holds `Rc<Vec<NamedMcpClient>>` (the McpClient is
    /// pinned to a single thread). Building on the worker side
    /// means the `Rc` is born and stays on that thread.
    ///
    /// Returns the worker handle on success, or the build error.
    fn spawn(builder: crate::shell::ShellBuilder) -> Result<Self> {
        let (tx, rx) = std_mpsc::channel::<Job>();
        // Build outcome travels back over a oneshot std_mpsc so we can
        // surface kernel-level build errors as a Promise rejection.
        let (build_tx, build_rx) = std_mpsc::channel::<std::io::Result<()>>();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime for strands-shell worker");
            // Build the shell *here* so the Rc inside it never crosses
            // a thread boundary. ShellBuilder::build is sync but it
            // wires Rc<NamedMcpClient> in, which is why we have to
            // build on the worker thread rather than ahead of time.
            let mut shell = match builder.build() {
                Ok(s) => {
                    let _ = build_tx.send(Ok(()));
                    s
                }
                Err(e) => {
                    let _ = build_tx.send(Err(e));
                    return;
                }
            };
            let _ = &runtime; // ensure runtime stays alive for jobs below
            while let Ok(job) = rx.recv() {
                job(&mut shell, &runtime);
            }
        });
        match build_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(e)) => Err(Error::from_reason(e.to_string())),
            Err(_) => Err(Error::from_reason(
                "strands-shell worker thread terminated during build",
            )),
        }
    }

    /// Submit a sync closure and await its result on a oneshot. The
    /// closure runs on the worker thread, so it has free access to the
    /// `!Send` shell. Returns whatever the closure produces.
    ///
    /// Both failure modes surface as a rejected Promise rather than a panic,
    /// so a dead worker can never abort the host Node process:
    ///
    /// * **Send fails** — the worker thread is already gone (e.g. a prior job
    ///   panicked and unwound it). The `Shell` is no longer usable.
    /// * **Recv fails** — the worker dropped the oneshot sender without
    ///   sending, which means *this* job panicked mid-flight and unwound the
    ///   worker thread. Again, the `Shell` is no longer usable.
    async fn run<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::Shell, &tokio::runtime::Runtime) -> R + Send + 'static,
    {
        let (otx, orx) = oneshot::channel::<R>();
        let job: Job = Box::new(move |shell, rt| {
            let result = f(shell, rt);
            // If the receiver was dropped (caller cancelled), we silently
            // discard the result — same as a fire-and-forget would do.
            let _ = otx.send(result);
        });
        self.tx.send(job).map_err(|_| {
            Error::from_reason("strands-shell worker thread is gone; the Shell is no longer usable")
        })?;
        orx.await
            .map_err(|_| Error::from_reason("strands-shell worker thread panicked while running a job; the Shell is no longer usable"))
    }
}

// ---------------------------------------------------------------------------
// Value classes — plain object shape, mirrored from src/python.rs
// ---------------------------------------------------------------------------

/// Output from a shell command execution.
///
/// Defined as `#[napi(object)]` so it surfaces in JS as a plain object
/// literal rather than a class. Easier to mock and `JSON.stringify`-friendly.
#[napi(object)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Metadata about a file or directory in the VFS.
///
/// Mirrors the eventual Strands TS Sandbox `FileInfo` shape so adapters
/// can spread by attribute copy.
#[napi(object)]
pub struct FileInfo {
    pub name: String,
    pub is_dir: Option<bool>,
    pub size: Option<u32>,
}

// ---------------------------------------------------------------------------
// Read-only config snapshot — plain object shapes mirrored from the core
// `crate::shell::{ShellConfig, BindInfo, CredInfo, LimitsInfo}` view types.
// Returned by `Shell.config()`. Secret values are never carried — see
// `CredInfo`.
// ---------------------------------------------------------------------------

/// A single bind mount in a config snapshot.
#[napi(object)]
pub struct BindInfo {
    pub source: String,
    pub destination: String,
    /// `"copy"` or `"direct"`.
    pub mode: String,
    pub readonly: bool,
}

/// A single credential rule in a config snapshot. Never carries the secret.
#[napi(object)]
pub struct CredInfo {
    pub url: String,
    /// `"bearer"` or `"query"`.
    pub kind: String,
    pub methods: Vec<String>,
    pub param: Option<String>,
    /// Name of the env var the secret is read from, or `null` for a literal.
    pub env_var: Option<String>,
    /// True when a literal token was supplied (value itself never exposed).
    pub from_literal: bool,
}

/// Resource caps in a config snapshot.
#[napi(object)]
pub struct LimitsInfo {
    // f64 because JS numbers are doubles; values fit comfortably (max_inodes
    // default 10_000, max_file_size default 10 MiB — well within 2^53).
    pub max_depth: f64,
    pub max_output: f64,
    pub max_fds: f64,
    pub max_bg_jobs: f64,
    pub max_pipeline: f64,
    pub max_input: f64,
    pub max_file_size: f64,
    pub max_inodes: f64,
}

/// A read-only snapshot of how a `Shell` was configured.
#[napi(object)]
pub struct ShellConfig {
    pub binds: Vec<BindInfo>,
    pub credentials: Vec<CredInfo>,
    pub allowed_urls: Vec<String>,
    /// Seeded environment variables as a plain object.
    pub env: std::collections::HashMap<String, String>,
    pub umask: f64,
    /// Per-command timeout in seconds, or `null` for no timeout.
    pub timeout: Option<f64>,
    pub limits: LimitsInfo,
}

// ---------------------------------------------------------------------------
// ShellBuilder
// ---------------------------------------------------------------------------

/// Builder for configuring a Shell.
///
/// The inner `crate::shell::ShellBuilder` lives behind a `Mutex<Option<...>>`
/// so we can take it out on `build()`. Re-using a consumed builder
/// throws `Error("builder consumed")`, matching the Python binding.
#[napi]
pub struct ShellBuilder {
    inner: Mutex<Option<crate::shell::ShellBuilder>>,
}

#[napi]
impl ShellBuilder {
    #[napi(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Some(crate::Shell::builder())),
        }
    }

    /// Apply a transformation to the inner builder. Helper used by every
    /// fluent setter to avoid repeating the lock+take+put-back dance.
    fn chain<F>(&self, f: F) -> Result<&Self>
    where
        F: FnOnce(crate::shell::ShellBuilder) -> crate::shell::ShellBuilder,
    {
        let mut guard = self.inner.lock().unwrap();
        let b = guard
            .take()
            .ok_or_else(|| Error::from_reason("builder consumed"))?;
        *guard = Some(f(b));
        Ok(self)
    }

    /// Bind a host path into the VFS (copy mode).
    #[napi]
    pub fn bind(&self, source: String, destination: String) -> Result<&Self> {
        self.chain(|b| b.bind(&source, &destination))
    }

    /// Bind a host path as read-only (copy mode).
    #[napi]
    pub fn bind_readonly(&self, source: String, destination: String) -> Result<&Self> {
        self.chain(|b| b.bind_readonly(&source, &destination))
    }

    /// Bind a host path with direct passthrough.
    #[napi]
    pub fn bind_direct(&self, source: String, destination: String) -> Result<&Self> {
        self.chain(|b| b.bind_direct(&source, &destination))
    }

    /// Bind a host path as read-only with direct passthrough.
    #[napi]
    pub fn bind_direct_readonly(&self, source: String, destination: String) -> Result<&Self> {
        self.chain(|b| b.bind_direct_readonly(&source, &destination))
    }

    /// Add a bearer token credential for URLs matching a pattern.
    #[napi]
    pub fn credential(&self, url_pattern: String, token: String) -> Result<&Self> {
        self.chain(|b| b.credential(&url_pattern, crate::CredKind::Bearer, &token))
    }

    /// Add a bearer token credential from an environment variable.
    #[napi]
    pub fn credential_from_env(&self, url_pattern: String, env_var: String) -> Result<&Self> {
        self.chain(|b| b.credential_from_env(&url_pattern, crate::CredKind::Bearer, &env_var))
    }

    /// Allow curl requests to URLs matching prefix (bypasses SSRF protection).
    #[napi]
    pub fn allow_url(&self, prefix: String) -> Result<&Self> {
        self.chain(|b| b.allow_url(&prefix))
    }

    /// Set an environment variable.
    #[napi]
    pub fn env(&self, key: String, value: String) -> Result<&Self> {
        self.chain(|b| b.env(&key, &value))
    }

    /// Set the umask for file creation (default: 0o022).
    #[napi]
    pub fn umask(&self, umask: u32) -> Result<&Self> {
        self.chain(|b| b.umask(umask))
    }

    /// Set timeout in seconds.
    #[napi]
    pub fn timeout(&self, seconds: f64) -> Result<&Self> {
        self.chain(|b| b.timeout(Duration::from_secs_f64(seconds)))
    }

    /// Set max recursion depth for functions/subshells.
    #[napi]
    pub fn max_depth(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_depth(n))
    }

    /// Set max output size in bytes.
    #[napi]
    pub fn max_output(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_output(n as usize))
    }

    /// Set max file size in bytes.
    #[napi]
    pub fn max_file_size(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_file_size(n as usize))
    }

    /// Set max open file descriptors.
    #[napi]
    pub fn max_fds(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_fds(n as usize))
    }

    /// Set max concurrent background jobs.
    #[napi]
    pub fn max_bg_jobs(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_bg_jobs(n as usize))
    }

    /// Set max pipeline stages.
    #[napi]
    pub fn max_pipeline(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_pipeline(n as usize))
    }

    /// Set max input size for parser in bytes.
    #[napi]
    pub fn max_input(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_input(n as usize))
    }

    /// Set max inodes (files + directories) in VFS.
    #[napi]
    pub fn max_inodes(&self, n: u32) -> Result<&Self> {
        self.chain(|b| b.max_inodes(n as usize))
    }

    /// Load config from a TOML file.
    #[napi]
    pub fn config_file(&self, path: String) -> Result<&Self> {
        let mut guard = self.inner.lock().unwrap();
        let b = guard
            .take()
            .ok_or_else(|| Error::from_reason("builder consumed"))?;
        let updated = b
            .config_file(&path)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        *guard = Some(updated);
        Ok(self)
    }

    /// Build the Shell. Async because mount materialization may do I/O.
    ///
    /// The actual build runs on the new shell's dedicated worker thread
    /// — see `Worker::spawn`. We hop onto napi's blocking thread pool
    /// only to avoid stalling the JS event loop while waiting for the
    /// worker's build oneshot.
    #[napi]
    pub async fn build(&self) -> Result<Shell> {
        let builder = {
            let mut guard = self.inner.lock().unwrap();
            guard
                .take()
                .ok_or_else(|| Error::from_reason("builder consumed"))?
        };
        let worker = napi::tokio::task::spawn_blocking(move || Worker::spawn(builder))
            .await
            .map_err(|e| Error::from_reason(format!("build worker join error: {e}")))??;
        Ok(Shell { worker })
    }
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// A sandboxed shell environment.
#[napi]
pub struct Shell {
    worker: Worker,
}

#[napi]
impl Shell {
    /// Create a new ShellBuilder.
    #[napi]
    pub fn builder() -> ShellBuilder {
        ShellBuilder::new()
    }

    /// Run a command and capture output.
    #[napi]
    pub async fn run(&self, command: String) -> Result<Output> {
        self.worker
            .run(move |shell, rt| {
                let local = tokio::task::LocalSet::new();
                let out = rt.block_on(local.run_until(shell.run(&command)));
                Output {
                    status: out.status,
                    stdout: out.stdout,
                    stderr: out.stderr,
                }
            })
            .await
    }

    /// Set an environment variable.
    #[napi]
    pub async fn set_env(&self, key: String, value: String) -> Result<()> {
        self.worker
            .run(move |shell, _rt| {
                shell.set_env(&key, &value);
            })
            .await?;
        Ok(())
    }

    /// Get an environment variable.
    #[napi]
    pub async fn get_env(&self, key: String) -> Result<Option<String>> {
        self.worker
            .run(move |shell, _rt| shell.get_env(&key).map(|s| s.to_string()))
            .await
    }

    /// Read-only snapshot of the configuration this shell was built with.
    ///
    /// Mirrors `Shell::config()` in the core. Never carries secret values —
    /// each credential reports its source (literal vs env-var name) only.
    #[napi]
    pub async fn config(&self) -> Result<ShellConfig> {
        self.worker
            .run(move |shell, _rt| {
                let c = shell.config();
                ShellConfig {
                    binds: c
                        .binds
                        .iter()
                        .map(|b| BindInfo {
                            source: b.source.clone(),
                            destination: b.destination.clone(),
                            mode: b.mode.to_string(),
                            readonly: b.readonly,
                        })
                        .collect(),
                    credentials: c
                        .credentials
                        .iter()
                        .map(|cr| CredInfo {
                            url: cr.url.clone(),
                            kind: cr.kind.to_string(),
                            methods: cr.methods.clone(),
                            param: cr.param.clone(),
                            env_var: cr.env_var.clone(),
                            from_literal: cr.from_literal,
                        })
                        .collect(),
                    allowed_urls: c.allowed_urls.clone(),
                    env: c.env.iter().cloned().collect(),
                    umask: c.umask as f64,
                    timeout: c.timeout_secs,
                    limits: LimitsInfo {
                        max_depth: c.limits.max_depth as f64,
                        max_output: c.limits.max_output as f64,
                        max_fds: c.limits.max_fds as f64,
                        max_bg_jobs: c.limits.max_bg_jobs as f64,
                        max_pipeline: c.limits.max_pipeline as f64,
                        max_input: c.limits.max_input as f64,
                        max_file_size: c.limits.max_file_size as f64,
                        max_inodes: c.limits.max_inodes as f64,
                    },
                }
            })
            .await
    }

    /// Read a file from the virtual filesystem as raw bytes.
    #[napi]
    pub async fn read_file(&self, path: String) -> Result<Uint8Array> {
        let path_for_err = path.clone();
        let result = self
            .worker
            .run(
                move |shell, rt| -> std::result::Result<Vec<u8>, std::io::Error> {
                    let local = tokio::task::LocalSet::new();
                    rt.block_on(local.run_until(shell.read_file(&path)))
                },
            )
            .await?;
        match result {
            Ok(bytes) => Ok(Uint8Array::from(bytes)),
            Err(e) => Err(file_error(&path_for_err, &e)),
        }
    }

    /// Write raw bytes to a file in the virtual filesystem.
    ///
    /// Creates parent directories if missing. Truncates any existing file.
    #[napi]
    pub async fn write_file(&self, path: String, content: Uint8Array) -> Result<()> {
        // Copy bytes off the napi-managed Uint8Array; the closure must
        // be 'static so it can travel to the worker thread.
        let bytes: Vec<u8> = content.to_vec();
        let path_for_err = path.clone();
        let result = self
            .worker
            .run(
                move |shell, rt| -> std::result::Result<(), std::io::Error> {
                    let local = tokio::task::LocalSet::new();
                    rt.block_on(local.run_until(shell.write_file(&path, &bytes)))
                },
            )
            .await?;
        result.map_err(|e| file_error(&path_for_err, &e))
    }

    /// Remove a file from the virtual filesystem.
    #[napi]
    pub async fn remove_file(&self, path: String) -> Result<()> {
        let path_for_err = path.clone();
        let result = self
            .worker
            .run(
                move |shell, rt| -> std::result::Result<(), std::io::Error> {
                    let local = tokio::task::LocalSet::new();
                    rt.block_on(local.run_until(shell.remove_file(&path)))
                },
            )
            .await?;
        result.map_err(|e| file_error(&path_for_err, &e))
    }

    /// List entries in a directory, returning structured `FileInfo` objects.
    ///
    /// Names are basenames (no leading path).
    #[napi]
    pub async fn list_files(&self, path: String) -> Result<Vec<FileInfo>> {
        let path_for_err = path.clone();
        let result = self
            .worker
            .run(
                move |shell, rt| -> std::result::Result<Vec<crate::shell::FileInfo>, std::io::Error> {
                    let local = tokio::task::LocalSet::new();
                    rt.block_on(local.run_until(shell.list_files(&path)))
                },
            )
            .await?;
        result
            .map(|infos| {
                infos
                    .into_iter()
                    .map(|f| FileInfo {
                        name: f.name,
                        is_dir: f.is_dir,
                        // u64 → u32 truncation: VFS sizes are bounded by
                        // max_file_size (default 10 MiB). u32::MAX is 4 GiB,
                        // so we only lose precision on absurd configurations;
                        // saturate to be safe.
                        size: f.size.map(|n| n.min(u32::MAX as u64) as u32),
                    })
                    .collect()
            })
            .map_err(|e| file_error(&path_for_err, &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job that panics must unwind only the worker thread and surface as an
    /// `Err` from `run()` — never abort the host process. This is the safety
    /// guarantee the napi wrappers rely on to turn a dead worker into a
    /// rejected Promise instead of a process crash.
    #[tokio::test]
    async fn panicking_job_rejects_instead_of_aborting() {
        let worker = Worker::spawn(crate::Shell::builder()).expect("worker should build");

        // First call panics inside the job. We expect an Err, and crucially
        // the test process keeps running (no abort).
        let panicked: Result<()> = worker
            .run(|_shell, _rt| {
                panic!("boom — simulated job panic");
            })
            .await;
        assert!(panicked.is_err(), "a panicking job must surface as Err");

        // The worker thread is now gone. A subsequent call must also Err
        // (send fails) rather than hang or panic.
        let after: Result<()> = worker.run(|_shell, _rt| {}).await;
        assert!(
            after.is_err(),
            "calls after the worker dies must surface as Err"
        );
    }

    /// The happy path still returns the closure's value through the new
    /// `Result` wrapper.
    #[tokio::test]
    async fn normal_job_returns_value() {
        let worker = Worker::spawn(crate::Shell::builder()).expect("worker should build");
        let got: Result<u32> = worker.run(|_shell, _rt| 42).await;
        assert_eq!(got.unwrap(), 42);
    }
}
