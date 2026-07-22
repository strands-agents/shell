//! Builder-based API for creating and running sandboxed shells.
//!
//! This is the primary public interface for the crate. Start with
//! [`Shell::builder()`] to configure a shell, then call [`Shell::run()`]
//! or [`Shell::execute()`] to run commands.
//!
//! See the [crate-level documentation](crate) for a full overview.

use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::exec;
#[cfg(not(target_arch = "wasm32"))]
use crate::mcp_client::{McpConfigEntry, NamedMcpClient};
use crate::os::{Kernel, OpenFlags, Process};
use crate::vfs_config::{
    BindEntry, BindMode, CredEntry, CredKind, VfsConfig, build_vfs, resolve_creds,
};
use crate::vfs_kernel::VfsKernel;

/// Structured output from a shell command execution.
///
/// Returned by [`Shell::run()`], which captures both stdout and stderr.
///
/// ```rust,no_run
/// # async fn example() -> std::io::Result<()> {
/// # let mut shell = strands_shell::Shell::builder().build()?;
/// let output = shell.run("echo hello && echo oops >&2").await;
/// assert_eq!(output.status, 0);
/// assert_eq!(output.stdout.trim(), "hello");
/// assert_eq!(output.stderr.trim(), "oops");
/// # Ok(())
/// # }
/// ```
pub struct Output {
    /// Exit code of the command (0 = success).
    pub status: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

/// Metadata about a single VFS entry returned by [`Shell::list_files()`].
///
/// Mirrors the `FileInfo` shape used by the Strands `Sandbox` ABC and by
/// the `strands_shell` Python and `@strands-agents/shell` Node bindings, so adapter
/// code at the binding layer is a `From` conversion away.
///
/// `FileInfo` is `#[non_exhaustive]` so future kernels can carry richer
/// metadata (e.g. `mtime`) without breaking external callers' pattern
/// matches or struct literals.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FileInfo {
    /// Basename of the entry — no leading path.
    pub name: String,
    /// `Some(true)` for directories, `Some(false)` for files. `None` is
    /// part of the type because the bindings expose it as optional — Python
    /// (`is_dir: bool | None`) and JS (`isDir?: boolean`, i.e. `undefined`
    /// when unknown, matching the sandbox-provider contract). In practice
    /// today it is always `Some(_)`.
    pub is_dir: Option<bool>,
    /// Size in bytes for files, `None` for directories.
    pub size: Option<u64>,
}

/// A read-only snapshot of how a [`Shell`] was configured.
///
/// Captured at [`build()`](ShellBuilder::build) time and returned by
/// [`Shell::config()`]. This exists so an embedder (for example a sandbox
/// adapter in another SDK) can introspect a constructed `Shell` after the
/// fact — to build tool descriptions, surface the network allowlist, or
/// report the active resource caps — without having held onto the builder.
///
/// The snapshot intentionally **never carries resolved secret values**.
/// strands-shell's security model is that the agent never sees credentials,
/// so [`CredInfo`] reports only the URL pattern, the credential *kind*, and
/// the *source* of the secret (a literal was provided, or the name of the
/// environment variable it is read from) — never the token itself.
///
/// `#[non_exhaustive]` so future fields can be added without breaking callers
/// who construct or pattern-match exhaustively.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ShellConfig {
    /// Bind mounts mapping host paths into the VFS, in declaration order.
    pub binds: Vec<BindInfo>,
    /// Credential injection rules, in declaration order. Secret values are
    /// never included — see [`CredInfo`].
    pub credentials: Vec<CredInfo>,
    /// SSRF allowlist: URL prefixes `curl` may reach, in declaration order.
    pub allowed_urls: Vec<String>,
    /// Environment variables seeded into the shell, in declaration order.
    pub env: Vec<(String, String)>,
    /// File-creation umask.
    pub umask: u32,
    /// Per-command wall-clock timeout in seconds, or `None` for no timeout.
    pub timeout_secs: Option<f64>,
    /// Active resource caps.
    pub limits: LimitsInfo,
}

/// A single bind mount in a [`ShellConfig`] snapshot.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BindInfo {
    /// Host path that was mounted.
    pub source: String,
    /// Destination path inside the VFS.
    pub destination: String,
    /// `"copy"` (build-time snapshot) or `"direct"` (host passthrough).
    pub mode: &'static str,
    /// Whether writes through this mount are rejected.
    pub readonly: bool,
}

/// A single credential rule in a [`ShellConfig`] snapshot.
///
/// Carries everything an embedder needs to reason about a credential —
/// *except the secret itself*. When the credential was configured from an
/// environment variable, [`env_var`](Self::env_var) holds that variable's
/// name; the value is never read into the snapshot. When a literal token was
/// supplied, [`from_literal`](Self::from_literal) is `true` and `env_var` is
/// `None`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CredInfo {
    /// URL pattern the credential applies to (supports glob patterns).
    pub url: String,
    /// `"bearer"` or `"query"`.
    pub kind: &'static str,
    /// HTTP methods this credential is scoped to (empty means all methods).
    pub methods: Vec<String>,
    /// Query-parameter name, set only for `kind == "query"` credentials.
    pub param: Option<String>,
    /// Name of the environment variable the secret is read from, or `None`
    /// when a literal token was supplied.
    pub env_var: Option<String>,
    /// `true` when a literal token value was supplied directly (rather than
    /// via an environment variable). The token value itself is never exposed.
    pub from_literal: bool,
}

/// The resource caps active on a [`Shell`], as reported in a
/// [`ShellConfig`] snapshot.
///
/// Unlike [`crate::os::ProcessLimits`] (process-only), this view also carries
/// the two VFS-level caps (`max_file_size`, `max_inodes`) so the snapshot
/// reflects every limit the builder applied in one place.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct LimitsInfo {
    /// Max recursion depth for functions/subshells.
    pub max_depth: u32,
    /// Max size in bytes for any single output accumulation.
    pub max_output: usize,
    /// Max open file descriptors per process.
    pub max_fds: usize,
    /// Max concurrent background jobs.
    pub max_bg_jobs: usize,
    /// Max stages in a single pipeline.
    pub max_pipeline: usize,
    /// Max input size in bytes the parser will accept.
    pub max_input: usize,
    /// Max size in bytes for any single file in the VFS.
    pub max_file_size: usize,
    /// Max inodes (files + directories) in the VFS.
    pub max_inodes: usize,
}

impl Default for LimitsInfo {
    /// Matches [`ShellBuilder::default`]'s caps, so a [`Shell`] built without
    /// touching the limit setters reports these values.
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_output: 1024 * 1024,
            max_fds: 128,
            max_bg_jobs: 8,
            max_pipeline: 16,
            max_input: 1024 * 1024,
            max_file_size: 10 * 1024 * 1024,
            max_inodes: 10_000,
        }
    }
}

impl Default for ShellConfig {
    /// An empty snapshot with default umask, no timeout, and default caps.
    /// Used for shells created via [`Shell::with_kernel`], which bypass the
    /// builder and therefore have no captured configuration.
    fn default() -> Self {
        Self {
            binds: Vec::new(),
            credentials: Vec::new(),
            allowed_urls: Vec::new(),
            env: Vec::new(),
            umask: 0o022,
            timeout_secs: None,
            limits: LimitsInfo::default(),
        }
    }
}

/// Classification of a file-op `io::Error` into the categories the language
/// bindings surface as typed errors.
///
/// The kernel reports failures as [`io::Error`] values: most carry a precise
/// [`io::ErrorKind`] (`NotFound`, `PermissionDenied`), but the size/inode caps
/// use `ErrorKind::Other` with a diagnostic message. This enum is the single
/// place that classification logic lives, so the Python and JS bindings stay
/// in lockstep (`FileNotFoundError` / `NotFoundError`, `PermissionDeniedError`,
/// `FileTooLargeError`, and a generic base for everything else).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpErrorKind {
    /// Path missing — `io::ErrorKind::NotFound`.
    NotFound,
    /// Read-only mount or otherwise blocked — `io::ErrorKind::PermissionDenied`.
    PermissionDenied,
    /// `max_file_size` / `max_inodes` cap (on write or read), or a stalled
    /// drain consistent with the size cap.
    TooLarge,
    /// Anything else (not-a-directory, parent-is-a-file, host I/O, …).
    Other,
}

impl FileOpErrorKind {
    /// Classify a file-op `io::Error`. Pure and side-effect free so both
    /// bindings can call it on the error the core returns.
    pub fn classify(err: &io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => {
                // Size/inode caps surface as ErrorKind::Other with a known
                // message; match on the substrings the kernel emits.
                let msg = err.to_string();
                if msg.contains("file size limit")
                    || msg.contains("inode limit")
                    || msg.contains("write did not commit")
                {
                    Self::TooLarge
                } else {
                    Self::Other
                }
            }
        }
    }
}

/// A sandboxed shell environment.
///
/// `Shell` is the main entry point for running commands. Create one with
/// [`Shell::builder()`], then use [`run()`](Shell::run) to capture output
/// or [`execute()`](Shell::execute) for pass-through execution.
///
/// The shell maintains persistent state between commands — environment
/// variables, the current directory, and shell functions all carry over,
/// just like an interactive session.
///
/// # Examples
///
/// Basic usage:
///
/// ```rust,no_run
/// # async fn example() -> std::io::Result<()> {
/// use strands_shell::Shell;
///
/// let mut shell = Shell::builder().build()?;
///
/// // Commands share state
/// shell.run("cd /tmp").await;
/// shell.run("X=42").await;
/// let output = shell.run("echo $X from $PWD").await;
/// assert_eq!(output.stdout.trim(), "42 from /tmp");
/// # Ok(())
/// # }
/// ```
///
/// Sandboxed with bind mounts and limits:
///
/// ```rust,no_run
/// # async fn example() -> std::io::Result<()> {
/// use std::time::Duration;
/// use strands_shell::Shell;
///
/// let mut shell = Shell::builder()
///     .bind("/home/user/project", "/workspace")
///     .timeout(Duration::from_secs(30))
///     .max_depth(64)
///     .build()?;
///
/// let output = shell.run("grep -rn TODO /workspace").await;
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
pub struct Shell {
    kernel: Arc<dyn Kernel>,
    /// The shell process state.
    ///
    /// Exposed for advanced use cases that need direct access to the
    /// process, such as interactive REPLs using
    /// [`exec::execute_with_reader()`](crate::exec::execute_with_reader).
    /// Most users should use [`run()`](Shell::run) or
    /// [`execute()`](Shell::execute) instead.
    pub proc: Process,
    /// Configured per-command timeout. Used to refresh `proc.deadline`
    /// on every `run()` / `execute()` call so that idle time between
    /// commands does not eat into the per-command budget.
    timeout: Option<Duration>,
    /// `max_file_size` cap (bytes) applied to `read_file`, so a read can
    /// never pull more into memory than a write is allowed to commit.
    /// `0` means no cap. Mirrors the kernel's write-side `max_file_size`.
    max_file_size: usize,
    /// Read-only snapshot of the configuration this shell was built with.
    /// Captured at build time so embedders can introspect a constructed
    /// shell (see [`Shell::config`]). Never carries secret values.
    config: ShellConfig,
    #[cfg(not(target_arch = "wasm32"))]
    mcp_clients: Rc<Vec<NamedMcpClient>>,
    #[cfg(not(target_arch = "wasm32"))]
    mcp_config: Vec<McpConfigEntry>,
}

impl Shell {
    /// Create a new [`ShellBuilder`] for configuring a shell.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> std::io::Result<()> {
    /// let mut shell = strands_shell::Shell::builder()
    ///     .bind("/host/path", "/vfs/path")
    ///     .env("MY_VAR", "my_value")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder() -> ShellBuilder {
        ShellBuilder::default()
    }

    /// Create a shell from a custom [`Kernel`]
    /// implementation.
    ///
    /// Use this when you need a backend other than the built-in VFS —
    /// for example, one backed by S3, a database, or a remote API.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use std::sync::Arc;
    /// use strands_shell::Shell;
    /// use strands_shell::os::Kernel;
    ///
    /// fn create_shell(kernel: Arc<dyn Kernel>) -> Shell {
    ///     Shell::with_kernel(kernel)
    /// }
    /// ```
    pub fn with_kernel(kernel: Arc<dyn Kernel>) -> Self {
        let proc = kernel.new_process();
        Self {
            kernel,
            proc,
            timeout: None,
            max_file_size: 0,
            config: ShellConfig::default(),
            #[cfg(not(target_arch = "wasm32"))]
            mcp_clients: Rc::new(Vec::new()),
            #[cfg(not(target_arch = "wasm32"))]
            mcp_config: Vec::new(),
        }
    }

    /// Refresh `proc.deadline` to `now + timeout` so the per-command
    /// budget starts fresh on each `run()` / `execute()`.
    fn refresh_deadline(&mut self) {
        if let Some(dur) = self.timeout {
            #[cfg(not(target_arch = "wasm32"))]
            {
                self.proc.deadline = Some(tokio::time::Instant::now() + dur);
            }
            #[cfg(target_arch = "wasm32")]
            {
                self.proc.deadline = Some(std::time::Instant::now() + dur);
            }
        }
    }

    /// Run a command and capture its output.
    ///
    /// Both stdout and stderr are captured into the returned [`Output`].
    /// Nothing is printed to the real terminal. The shell's state
    /// (environment, cwd, functions) persists after the call.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> std::io::Result<()> {
    /// # let mut shell = strands_shell::Shell::builder().build()?;
    /// let output = shell.run("echo hello | tr a-z A-Z").await;
    /// assert_eq!(output.status, 0);
    /// assert_eq!(output.stdout.trim(), "HELLO");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(&mut self, input: &str) -> Output {
        self.refresh_deadline();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.start_mcp().await;
            crate::io::set_mcp_clients(self.mcp_clients.clone());
        }
        let (status, stdout, stderr) =
            exec::execute_capture(self.kernel.clone(), &mut self.proc, input).await;
        Output {
            status,
            stdout,
            stderr,
        }
    }

    /// Execute a command, returning just the exit code.
    ///
    /// Unlike [`run()`](Shell::run), stdout and stderr are **not**
    /// captured — they flow to the real file descriptors. Use this for
    /// interactive or streaming output.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> std::io::Result<()> {
    /// # let mut shell = strands_shell::Shell::builder().build()?;
    /// let status = shell.execute("ls -la /").await;
    /// // Output was printed directly to the terminal
    /// assert_eq!(status, 0);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(&mut self, input: &str) -> i32 {
        self.refresh_deadline();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.start_mcp().await;
            crate::io::set_mcp_clients(self.mcp_clients.clone());
        }
        let (code, _) = exec::execute(self.kernel.clone(), &mut self.proc, input).await;
        code
    }

    /// Set an environment variable in the shell.
    ///
    /// This is equivalent to running `export KEY=VALUE` inside the shell.
    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.proc.set_env(key, value);
    }

    /// Get an environment variable from the shell.
    pub fn get_env(&self, key: &str) -> Option<&str> {
        self.proc.get_env(key)
    }

    /// Access the underlying [`Kernel`].
    ///
    /// Useful for advanced operations like passing the kernel to
    /// [`mcp::serve()`](crate::mcp::serve) or sharing it across
    /// multiple shells.
    pub fn kernel(&self) -> &Arc<dyn Kernel> {
        &self.kernel
    }

    /// Get the configured resource limits for this shell.
    ///
    /// Useful for passing limits to [`mcp::serve()`](crate::mcp::serve)
    /// so per-request processes inherit the same limits.
    pub fn limits(&self) -> crate::os::ProcessLimits {
        self.proc.limits()
    }

    /// Get a read-only snapshot of the configuration this shell was built
    /// with.
    ///
    /// The snapshot is captured at [`build()`](ShellBuilder::build) time and
    /// reports bind mounts, credential rules, the network allowlist, seeded
    /// environment variables, umask, timeout, and resource caps. It exists so
    /// an embedder (e.g. a sandbox adapter in another SDK) can introspect a
    /// constructed `Shell` without having held onto the builder — to build
    /// tool descriptions, surface the allowlist, or report active limits.
    ///
    /// Secret values are never included: [`CredInfo`] reports the credential's
    /// URL pattern, kind, and source (literal vs environment variable name),
    /// but never the token itself.
    ///
    /// Shells created via [`with_kernel`](Shell::with_kernel) (which bypass the
    /// builder) report a [`ShellConfig::default`] snapshot.
    pub fn config(&self) -> &ShellConfig {
        &self.config
    }

    /// Read a file from the virtual filesystem as raw bytes.
    ///
    /// Subject to the per-`Shell` `max_file_size` limit set on the builder,
    /// so a read can never pull more into memory than a write may commit.
    ///
    /// # Errors
    ///
    /// Returns `Err(io::Error)` if the path is missing, points to a
    /// directory, or the read exceeds `max_file_size`. The error message
    /// is prefixed with the path: `"{path}: {kernel diagnostic}"`.
    ///
    /// # Panics
    ///
    /// Must be called inside `LocalSet::run_until(...)` on a current-thread
    /// Tokio runtime; the underlying VFS uses `tokio::task::spawn_local`
    /// for its drain task and panics outside that context.
    pub async fn read_file(&mut self, path: &str) -> io::Result<Vec<u8>> {
        async fn inner(
            kernel: &Arc<dyn Kernel>,
            proc: &mut Process,
            path: &str,
            limit: usize,
        ) -> io::Result<Vec<u8>> {
            let fd = kernel.open(proc, path, OpenFlags::read()).await?;
            let mut reader = proc.take_reader(fd)?;
            // Bound the read by max_file_size so a read can never pull more
            // into memory than a write is allowed to commit — and so a
            // direct-passthrough mount can't surface an arbitrarily large
            // host file. A limit of 0 means "no cap" (see read_to_end_limited).
            // The limit-exceeded message classifies as TooLarge.
            crate::os::read_to_end_limited(&mut reader, limit)
                .await
                .map_err(|e| {
                    if e.kind() == io::ErrorKind::Other
                        && e.to_string().contains("output size limit")
                    {
                        io::Error::other("file size limit exceeded")
                    } else {
                        e
                    }
                })
        }
        let limit = self.max_file_size;
        inner(&self.kernel, &mut self.proc, path, limit)
            .await
            .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))
    }

    /// Write raw bytes to a file in the virtual filesystem.
    ///
    /// Creates missing parent directories (mkdir -p semantics) and
    /// truncates any existing file. Empty payloads (`b""`) produce a
    /// zero-byte file. Waits for the kernel's drain task to commit the
    /// write before returning.
    ///
    /// # Errors
    ///
    /// Returns `Err(io::Error)` if the parent path is a file, the mount
    /// is read-only, the write exceeds `max_file_size`, the VFS exceeds
    /// `max_inodes`, or the drain task stalls. The error message is
    /// prefixed with the path: `"{path}: {kernel diagnostic}"`.
    ///
    /// # Panics
    ///
    /// Must be called inside `LocalSet::run_until(...)` on a current-thread
    /// Tokio runtime.
    pub async fn write_file(&mut self, path: &str, content: &[u8]) -> io::Result<()> {
        let kernel = &self.kernel;
        let proc = &mut self.proc;
        let expected = content.len() as u64;
        let result: io::Result<()> = async {
            if let Some(parent) = parent_dir(path) {
                create_dir_recursive(kernel.as_ref(), proc, &parent).await?;
            }
            let fd = kernel.open(proc, path, OpenFlags::write()).await?;
            {
                // Scope the writer so it is dropped (closing the channel)
                // before we wait for the kernel's background drain task.
                let mut writer = proc.take_writer(fd)?;
                writer.write_all(content).await?;
                writer.shutdown().await?;
            }
            // Bound by "no progress for STALL_LIMIT consecutive yields"
            // rather than a fixed iteration count, so arbitrarily large
            // writes converge as long as the drain task keeps making
            // progress. If progress stalls before reaching `expected`,
            // the kernel almost certainly hit `max_file_size`.
            const STALL_LIMIT: u32 = 1024;
            let mut last_len = u64::MAX;
            let mut stalled = 0u32;
            loop {
                tokio::task::yield_now().await;
                let s = kernel.stat(proc, path).await;
                if s.exists && s.len == expected {
                    return Ok(());
                }
                let cur = if s.exists { s.len } else { 0 };
                if cur != last_len {
                    last_len = cur;
                    stalled = 0;
                } else {
                    stalled += 1;
                    if stalled >= STALL_LIMIT {
                        return Err(io::Error::other(
                            "write did not commit (file size limit exceeded?)",
                        ));
                    }
                }
            }
        }
        .await;
        result.map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))
    }

    /// Remove a file from the virtual filesystem.
    ///
    /// Errors if the path is a directory or does not exist. Use
    /// `shell.run("rm -rf ...")` for recursive directory removal.
    ///
    /// # Errors
    ///
    /// Returns `Err(io::Error)` prefixed with `"{path}: ..."`.
    pub async fn remove_file(&mut self, path: &str) -> io::Result<()> {
        self.kernel
            .remove_file(&self.proc, path)
            .await
            .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))
    }

    /// List the entries in a directory.
    ///
    /// `FileInfo.name` is the basename only — `"x.txt"`, never
    /// `"/work/x.txt"`. `FileInfo.size` is `None` for directories,
    /// `Some(bytes)` for files.
    ///
    /// # Errors
    ///
    /// Returns `Err(io::Error)` prefixed with `"{path}: ..."` if the
    /// path is missing or not a directory.
    pub async fn list_files(&mut self, path: &str) -> io::Result<Vec<FileInfo>> {
        let entries = self
            .kernel
            .list_dir(&self.proc, path)
            .await
            .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))?;

        let base = if path.ends_with('/') {
            path.trim_end_matches('/').to_string()
        } else {
            path.to_string()
        };

        let mut out = Vec::with_capacity(entries.len());
        for e in entries {
            let child = if base.is_empty() || base == "/" {
                format!("/{}", e.name)
            } else {
                format!("{}/{}", base, e.name)
            };
            let stat = self.kernel.stat(&self.proc, &child).await;
            let size = if stat.exists && !e.is_dir {
                Some(stat.len)
            } else {
                None
            };
            out.push(FileInfo {
                name: e.name,
                is_dir: Some(e.is_dir),
                size,
            });
        }
        Ok(out)
    }

    /// Start any configured MCP servers that haven't been started yet.
    ///
    /// This is called automatically by [`run()`](Shell::run) and
    /// [`execute()`](Shell::execute), but can be called explicitly
    /// to start servers eagerly (e.g. before an interactive REPL).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn start_mcp(&mut self) {
        if self.mcp_config.is_empty() {
            return;
        }
        let entries = std::mem::take(&mut self.mcp_config);
        match crate::mcp_client::start_clients(&entries).await {
            Ok(clients) => {
                self.mcp_clients = Rc::new(clients);
                crate::io::set_mcp_clients(self.mcp_clients.clone());
            }
            Err(e) => eprintln!("strands-shell: mcp: {e}"),
        }
    }
}

/// Builder for configuring and constructing a [`Shell`].
///
/// The builder configures three aspects of the shell:
///
/// 1. **Filesystem** — bind mounts that expose host paths into the
///    virtual filesystem
/// 2. **Network** — credentials injected into HTTP requests
/// 3. **Limits** — resource constraints to prevent runaway execution
///
/// # Bind Mount Modes
///
/// | Method | Behavior |
/// |--------|----------|
/// | [`bind()`](Self::bind) | Copies files into the VFS at build time (isolated snapshot) |
/// | [`bind_direct()`](Self::bind_direct) | Passes reads/writes through to the host filesystem |
/// | [`bind_readonly()`](Self::bind_readonly) | Copy mode, read-only in the VFS |
/// | [`bind_direct_readonly()`](Self::bind_direct_readonly) | Direct passthrough, read-only |
///
/// Copy mode is safer (the agent can't modify host files) but uses
/// memory proportional to file size. Direct mode is zero-copy but
/// gives the agent real filesystem access to that path.
///
/// # Example
///
/// ```rust,no_run
/// # async fn example() -> std::io::Result<()> {
/// use std::time::Duration;
/// use strands_shell::{CredKind, Shell};
///
/// let mut shell = Shell::builder()
///     // Filesystem
///     .bind("/home/user/project/src", "/workspace/src")
///     .bind_direct("/tmp/output", "/output")
///     // Network
///     .credential_from_env(
///         "https://api.example.com/*",
///         CredKind::Bearer,
///         "API_TOKEN",
///     )
///     // Limits
///     .timeout(Duration::from_secs(30))
///     .max_depth(64)
///     .max_output(1024 * 1024)
///     // Environment
///     .env("PROJECT", "my-project")
///     .umask(0o022)
///     .build()?;
///
/// let output = shell.run("ls /workspace/src").await;
/// # Ok(())
/// # }
/// ```
pub struct ShellBuilder {
    config: VfsConfig,
    creds: Vec<CredEntry>,
    env: Vec<(String, String)>,
    #[cfg(not(target_arch = "wasm32"))]
    mcp: Vec<McpConfigEntry>,
    max_depth: u32,
    max_output: usize,
    max_fds: usize,
    max_bg_jobs: usize,
    max_pipeline: usize,
    max_input: usize,
    max_file_size: usize,
    max_inodes: usize,
    timeout: Option<Duration>,
    allowed_url_prefixes: Vec<String>,
    /// Cedar policy source text, if any. Compiled into a `PolicyEngine` at
    /// [`build`](Self::build).
    #[cfg(not(target_arch = "wasm32"))]
    policy_text: Option<String>,
}

impl Default for ShellBuilder {
    fn default() -> Self {
        Self {
            config: VfsConfig::default(),
            creds: Vec::new(),
            env: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            mcp: Vec::new(),
            max_depth: 64,
            max_output: 1024 * 1024,
            max_fds: 128,
            max_bg_jobs: 8,
            max_pipeline: 16,
            max_input: 1024 * 1024,
            max_file_size: 10 * 1024 * 1024,
            max_inodes: 10_000,
            timeout: Some(std::time::Duration::from_secs(30)),
            allowed_url_prefixes: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            policy_text: None,
        }
    }
}

impl ShellBuilder {
    /// Bind a host path into the virtual filesystem using copy mode.
    ///
    /// The contents of `source` are copied into the VFS at `destination`
    /// when [`build()`](Self::build) is called. Changes inside the shell
    /// do not affect the host.
    ///
    /// `source` can be a file or directory. Directories are copied
    /// recursively.
    pub fn bind(mut self, source: impl Into<String>, destination: impl Into<String>) -> Self {
        self.config.bind.push(BindEntry {
            mode: BindMode::Copy,
            source: source.into(),
            destination: destination.into(),
            readonly: false,
        });
        self
    }

    /// Bind a host path as read-only using copy mode.
    ///
    /// Like [`bind()`](Self::bind), but the files cannot be modified
    /// inside the shell.
    pub fn bind_readonly(
        mut self,
        source: impl Into<String>,
        destination: impl Into<String>,
    ) -> Self {
        self.config.bind.push(BindEntry {
            mode: BindMode::Copy,
            source: source.into(),
            destination: destination.into(),
            readonly: true,
        });
        self
    }

    /// Bind a host path with direct passthrough.
    ///
    /// Reads and writes inside the shell go directly to the host
    /// filesystem. No data is copied into the VFS. This is useful for
    /// large directories or when you want the agent to produce output
    /// files on the host.
    pub fn bind_direct(
        mut self,
        source: impl Into<String>,
        destination: impl Into<String>,
    ) -> Self {
        self.config.bind.push(BindEntry {
            mode: BindMode::Direct,
            source: source.into(),
            destination: destination.into(),
            readonly: false,
        });
        self
    }

    /// Bind a host path as read-only with direct passthrough.
    ///
    /// Like [`bind_direct()`](Self::bind_direct), but writes are
    /// rejected.
    pub fn bind_direct_readonly(
        mut self,
        source: impl Into<String>,
        destination: impl Into<String>,
    ) -> Self {
        self.config.bind.push(BindEntry {
            mode: BindMode::Direct,
            source: source.into(),
            destination: destination.into(),
            readonly: true,
        });
        self
    }

    /// Add a credential for HTTP requests matching a URL pattern.
    ///
    /// When the shell executes `curl` against a URL matching `url`,
    /// the credential is injected as an HTTP header automatically.
    /// The `url` parameter supports glob patterns (e.g.
    /// `https://api.example.com/*`).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # fn example() {
    /// # let builder = strands_shell::Shell::builder();
    /// use strands_shell::CredKind;
    /// builder.credential(
    ///     "https://api.example.com/*",
    ///     CredKind::Bearer,
    ///     "sk-my-token",
    /// );
    /// # }
    /// ```
    pub fn credential(
        mut self,
        url: impl Into<String>,
        kind: CredKind,
        api_key: impl Into<String>,
    ) -> Self {
        self.creds.push(CredEntry {
            url: url.into(),
            methods: Vec::new(),
            kind,
            api_key: Some(api_key.into()),
            api_key_env: None,
            param: None,
        });
        self
    }

    /// Add a credential that reads the API key from an environment
    /// variable at build time.
    ///
    /// This avoids hardcoding secrets. The environment variable is
    /// read when [`build()`](Self::build) is called — if it is not
    /// set, `build()` returns an error.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # fn example() {
    /// # let builder = strands_shell::Shell::builder();
    /// use strands_shell::CredKind;
    /// builder.credential_from_env(
    ///     "https://api.openai.com/*",
    ///     CredKind::Bearer,
    ///     "OPENAI_API_KEY",
    /// );
    /// # }
    /// ```
    pub fn credential_from_env(
        mut self,
        url: impl Into<String>,
        kind: CredKind,
        env_var: impl Into<String>,
    ) -> Self {
        self.creds.push(CredEntry {
            url: url.into(),
            methods: Vec::new(),
            kind,
            api_key: None,
            api_key_env: Some(env_var.into()),
            param: None,
        });
        self
    }

    /// Set the umask for file creation (default: `0o022`).
    pub fn umask(mut self, umask: u32) -> Self {
        self.config.umask = umask;
        self
    }

    /// Set an environment variable that will be available in the shell.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set the maximum recursion depth for shell functions, subshells,
    /// and command substitutions (default: unlimited).
    pub fn max_depth(mut self, n: u32) -> Self {
        self.max_depth = n;
        self
    }

    /// Set the maximum size in bytes for any single output
    /// accumulation (default: unlimited).
    pub fn max_output(mut self, n: usize) -> Self {
        self.max_output = n;
        self
    }

    /// Set the maximum number of open file descriptors per process
    /// (default: unlimited).
    pub fn max_fds(mut self, n: usize) -> Self {
        self.max_fds = n;
        self
    }

    /// Set the maximum number of concurrent background jobs
    /// (default: unlimited).
    pub fn max_bg_jobs(mut self, n: usize) -> Self {
        self.max_bg_jobs = n;
        self
    }

    /// Set the maximum number of stages in a single pipeline
    /// (default: unlimited).
    pub fn max_pipeline(mut self, n: usize) -> Self {
        self.max_pipeline = n;
        self
    }

    /// Set the maximum input size in bytes that the parser will accept
    /// (default: unlimited).
    pub fn max_input(mut self, n: usize) -> Self {
        self.max_input = n;
        self
    }

    /// Set the maximum size in bytes for any single file in the VFS
    /// (default: unlimited).
    pub fn max_file_size(mut self, n: usize) -> Self {
        self.max_file_size = n;
        self
    }

    /// Set the maximum number of inodes (files + directories) in the
    /// VFS (default: unlimited).
    pub fn max_inodes(mut self, n: usize) -> Self {
        self.max_inodes = n;
        self
    }

    /// Set a per-command wall-clock timeout for this shell.
    ///
    /// The deadline is reset on every [`run()`](Shell::run) /
    /// [`execute()`](Shell::execute) call, so idle time between
    /// commands does not consume the budget. A command that runs longer
    /// than `duration` is terminated and its `Output` carries
    /// `status = 1` with `strands-shell: execution timeout exceeded` in stderr.
    ///
    /// A zero `duration` is rejected by [`build`](Self::build): there is no
    /// "unlimited" sentinel, so omit the timeout entirely for no limit.
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Allow curl requests to URLs matching the given prefix, bypassing
    /// the default SSRF protections. Useful for testing with local servers.
    pub fn allow_url(mut self, prefix: impl Into<String>) -> Self {
        self.allowed_url_prefixes.push(prefix.into());
        self
    }

    /// Load a Cedar authorization policy from a file.
    ///
    /// The policy is an *additional* restriction layer: with no policy, every
    /// operation is allowed (unchanged behavior); with a policy, gated actions
    /// (filesystem, network, env, MCP) must be permitted by the policy or they
    /// are denied. It never weakens the built-in SSRF / VFS-permission checks.
    /// The policy is parsed and schema-validated at [`build`](Self::build).
    ///
    /// See `schemas/agent.cedarschema` for the action vocabulary.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read. (Parse/validation errors
    /// surface from [`build`](Self::build).)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn policy_file(mut self, path: impl AsRef<Path>) -> io::Result<Self> {
        self.policy_text = Some(std::fs::read_to_string(path)?);
        Ok(self)
    }

    /// Set the Cedar authorization policy from a string. See
    /// [`policy_file`](Self::policy_file) for semantics.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn policy_str(mut self, text: impl Into<String>) -> Self {
        self.policy_text = Some(text.into());
        self
    }

    /// Load additional configuration from a TOML file.
    ///
    /// Bind mounts, credentials, and `allowed_urls` from the file are
    /// appended to whatever is already configured on the builder. The umask
    /// is overwritten. Resource caps under `[limits]` overwrite the
    /// corresponding builder values (an omitted key keeps the builder
    /// default). Environment variables follow a "code wins" rule: a key set
    /// explicitly via [`env`](Self::env) takes precedence over the same key
    /// in the file's `[env]` table, regardless of call order.
    ///
    /// See [`vfs_config::VfsConfig`](crate::vfs_config::VfsConfig) for
    /// the TOML format.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, contains invalid TOML,
    /// or contains an unknown key (typos fail the parse rather than being
    /// silently ignored).
    pub fn config_file(mut self, path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let config: VfsConfig = crate::vfs_config::parse_config(&content)?;
        self.config.bind.extend(config.bind);
        self.creds.extend(config.cred);
        #[cfg(not(target_arch = "wasm32"))]
        self.mcp.extend(config.mcp);
        self.config.umask = config.umask;
        self.allowed_url_prefixes.extend(config.allowed_urls);
        // A `policy = "file.cedar"` key points at a Cedar file resolved relative
        // to this config file's directory. Compiled/validated at build().
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rel) = config.policy {
            let policy_path = match path.parent() {
                Some(dir) if !rel.starts_with('/') => dir.join(&rel),
                _ => std::path::PathBuf::from(&rel),
            };
            self.policy_text = Some(std::fs::read_to_string(&policy_path)?);
        }
        // Env: an explicitly-passed `.env()` value always wins over the file,
        // regardless of whether `.env()` or `.config_file()` was called first
        // (matches the "code wins" rule for umask/timeout). Only take a TOML
        // entry whose key the builder doesn't already carry.
        for (k, v) in config.env {
            if !self.env.iter().any(|(existing, _)| existing == &k) {
                self.env.push((k, v));
            }
        }
        if let Some(limits) = config.limits {
            // Each cap is optional — an omitted TOML key leaves the builder
            // default untouched. config_file() routes process-level caps and
            // VFS-level caps (max_file_size / max_inodes) to their respective
            // builder fields; they're grouped under one [limits] table for the
            // user but applied to different subsystems at build time.
            if let Some(n) = limits.max_depth {
                self.max_depth = n;
            }
            if let Some(n) = limits.max_output {
                self.max_output = n;
            }
            if let Some(n) = limits.max_fds {
                self.max_fds = n;
            }
            if let Some(n) = limits.max_bg_jobs {
                self.max_bg_jobs = n;
            }
            if let Some(n) = limits.max_pipeline {
                self.max_pipeline = n;
            }
            if let Some(n) = limits.max_input {
                self.max_input = n;
            }
            if let Some(dur) = limits.timeout {
                self.timeout = Some(dur);
            }
            if let Some(n) = limits.max_file_size {
                self.max_file_size = n;
            }
            if let Some(n) = limits.max_inodes {
                self.max_inodes = n;
            }
        }
        Ok(self)
    }

    /// Build the [`Shell`].
    ///
    /// This resolves all credentials (reading environment variables as
    /// needed), constructs the virtual filesystem with bind mounts,
    /// and creates the initial shell process.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A bind mount source path does not exist
    /// - A credential references an environment variable that is not set
    /// - The configured timeout is zero (a zero timeout would expire every
    ///   command immediately; there is no "unlimited" sentinel — simply omit
    ///   the timeout for no limit)
    pub fn build(self) -> io::Result<Shell> {
        if self.timeout == Some(Duration::ZERO) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "timeout must be greater than zero (omit it for no timeout)",
            ));
        }

        // Capture a read-only config snapshot before the builder's fields are
        // moved into the kernel/process below. This is what `Shell::config()`
        // returns. Secret values are deliberately omitted: a credential
        // reports its source (literal vs env-var name) but never the token.
        let config_snapshot = ShellConfig {
            binds: self
                .config
                .bind
                .iter()
                .map(|b| BindInfo {
                    source: b.source.clone(),
                    destination: b.destination.clone(),
                    mode: b.mode.as_str(),
                    readonly: b.readonly,
                })
                .collect(),
            credentials: self
                .creds
                .iter()
                .map(|c| CredInfo {
                    url: c.url.clone(),
                    kind: c.kind.as_str(),
                    methods: c.methods.clone(),
                    param: c.param.clone(),
                    env_var: c.api_key_env.clone(),
                    from_literal: c.api_key.is_some(),
                })
                .collect(),
            allowed_urls: self.allowed_url_prefixes.clone(),
            env: self.env.clone(),
            umask: self.config.umask,
            timeout_secs: self.timeout.map(|d| d.as_secs_f64()),
            limits: LimitsInfo {
                max_depth: self.max_depth,
                max_output: self.max_output,
                max_fds: self.max_fds,
                max_bg_jobs: self.max_bg_jobs,
                max_pipeline: self.max_pipeline,
                max_input: self.max_input,
                max_file_size: self.max_file_size,
                max_inodes: self.max_inodes,
            },
        };

        let resolved_creds = resolve_creds(&self.creds)?;
        #[cfg(not(target_arch = "wasm32"))]
        let policy = match self.policy_text {
            Some(text) => Some(std::sync::Arc::new(crate::policy::PolicyEngine::from_str(
                &text,
            )?)),
            None => None,
        };
        let mut vfs = build_vfs(&self.config)?;
        vfs.max_file_size = self.max_file_size;
        vfs.max_inodes = self.max_inodes;
        let kernel: Arc<dyn Kernel> = Arc::new(VfsKernel {
            vfs: std::sync::Arc::new(tokio::sync::Mutex::new(vfs)),
            creds: resolved_creds,
            allowed_url_prefixes: self.allowed_url_prefixes,
            #[cfg(not(target_arch = "wasm32"))]
            policy,
        });
        let mut proc = kernel.new_process();

        proc.max_depth = self.max_depth;
        proc.max_output = self.max_output;
        proc.max_fds = self.max_fds;
        proc.max_bg_jobs = self.max_bg_jobs;
        proc.max_pipeline = self.max_pipeline;
        proc.max_input = self.max_input;
        proc.umask = self.config.umask;
        if let Some(dur) = self.timeout {
            #[cfg(not(target_arch = "wasm32"))]
            {
                proc.deadline = Some(tokio::time::Instant::now() + dur);
            }
            #[cfg(target_arch = "wasm32")]
            {
                proc.deadline = Some(std::time::Instant::now() + dur);
            }
        }

        for (k, v) in self.env {
            proc.set_env(k, v);
        }

        Ok(Shell {
            kernel,
            proc,
            timeout: self.timeout,
            max_file_size: self.max_file_size,
            config: config_snapshot,
            #[cfg(not(target_arch = "wasm32"))]
            mcp_clients: Rc::new(Vec::new()),
            #[cfg(not(target_arch = "wasm32"))]
            mcp_config: self.mcp,
        })
    }
}

/// Compute the parent directory of a path, or `None` if there is none
/// (root, empty, or no `/`).
fn parent_dir(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let idx = trimmed.rfind('/')?;
    if idx == 0 {
        None
    } else {
        Some(trimmed[..idx].to_string())
    }
}

/// Create a directory and all missing ancestors via the Kernel trait.
async fn create_dir_recursive(kernel: &dyn Kernel, proc: &Process, path: &str) -> io::Result<()> {
    let stat = kernel.stat(proc, path).await;
    if stat.exists && stat.is_dir {
        return Ok(());
    }
    if stat.exists {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{path}: not a directory"),
        ));
    }
    if let Some(parent) = parent_dir(path) {
        Box::pin(create_dir_recursive(kernel, proc, &parent)).await?;
    }
    kernel.create_dir(proc, path).await
}

#[cfg(test)]
mod tests {
    use super::parent_dir;

    #[test]
    fn parent_dir_of_root_is_none() {
        assert_eq!(parent_dir("/"), None);
    }

    #[test]
    fn parent_dir_of_empty_is_none() {
        assert_eq!(parent_dir(""), None);
    }

    #[test]
    fn parent_dir_of_top_level_is_none() {
        // "/foo" — parent is root, treated as None ("nothing to create").
        assert_eq!(parent_dir("/foo"), None);
    }

    #[test]
    fn parent_dir_of_nested_absolute_is_parent() {
        assert_eq!(parent_dir("/a/b/c"), Some("/a/b".to_string()));
    }

    #[test]
    fn parent_dir_strips_trailing_slash() {
        assert_eq!(parent_dir("/a/b/"), Some("/a".to_string()));
    }

    #[test]
    fn parent_dir_relative_path_is_supported() {
        assert_eq!(parent_dir("a/b/c"), Some("a/b".to_string()));
    }

    #[test]
    fn parent_dir_no_slash_is_none() {
        assert_eq!(parent_dir("foo"), None);
    }
}
