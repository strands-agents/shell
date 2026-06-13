use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::Bytes;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

/// Monotonic counter for virtual PIDs.
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// Metadata about a directory entry.
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// File metadata returned by Kernel::stat().
#[derive(Default)]
pub struct FileStat {
    pub exists: bool,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub len: u64,
    pub is_socket: bool,
    pub is_fifo: bool,
    pub is_block_device: bool,
    pub is_char_device: bool,
    /// Unix mode bits (permissions + setuid/setgid/sticky).
    pub mode: u32,
    /// Device ID (for -ef same-file check).
    pub dev: u64,
    /// Inode number (for -ef same-file check).
    pub ino: u64,
    /// Modification time as duration since epoch.
    pub modified: Option<std::time::SystemTime>,
}

/// Access permission modes for Kernel::access().
pub const ACCESS_R: i32 = 4;
pub const ACCESS_W: i32 = 2;
pub const ACCESS_X: i32 = 1;

/// File descriptor index.
pub type Fd = u32;

pub const STDIN: Fd = 0;
pub const STDOUT: Fd = 1;
pub const STDERR: Fd = 2;

/// Open flags for the open() syscall.
#[derive(Debug, Clone, Copy)]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub append: bool,
    pub truncate: bool,
}

impl OpenFlags {
    pub fn read() -> Self {
        Self {
            read: true,
            write: false,
            create: false,
            append: false,
            truncate: false,
        }
    }
    pub fn write() -> Self {
        Self {
            read: false,
            write: true,
            create: true,
            append: false,
            truncate: true,
        }
    }
    pub fn append() -> Self {
        Self {
            read: false,
            write: true,
            create: true,
            append: true,
            truncate: false,
        }
    }
}

/// The backing storage for a file descriptor.
pub enum FdKind {
    ChannelReader {
        rx: mpsc::Receiver<Bytes>,
        buf: Vec<u8>,
    },
    ChannelWriter {
        tx: mpsc::Sender<Bytes>,
        error_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    },
    #[cfg(not(target_arch = "wasm32"))]
    File(tokio::fs::File),
}

impl FdKind {
    async fn try_clone(&self) -> io::Result<FdKind> {
        match self {
            FdKind::ChannelWriter { tx, error_flag } => Ok(FdKind::ChannelWriter {
                tx: tx.clone(),
                error_flag: error_flag.clone(),
            }),
            #[cfg(not(target_arch = "wasm32"))]
            FdKind::File(f) => Ok(FdKind::File(f.try_clone().await?)),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cannot duplicate this fd",
            )),
        }
    }
}

/// Per-process state. Each shell/subshell gets its own.
pub struct Process {
    /// Virtual PID (not the real OS PID).
    pub pid: u32,
    pub cwd: PathBuf,
    pub env: Arc<HashMap<String, String>>,
    pub functions: Arc<HashMap<String, crate::parser::CommandLine>>,
    pub last_exit: i32,
    pub arg0: String,
    pub args: Vec<String>,
    /// Shell option flags.
    pub opt_errexit: bool,
    pub opt_nounset: bool,
    pub opt_xtrace: bool,
    /// Set when a nounset error occurs during expansion.
    pub nounset_error: bool,
    /// PID of last background job (for $!).
    pub last_bg_pid: Option<u32>,
    /// Background job handles.
    pub bg_jobs: Vec<tokio::task::JoinHandle<(i32, String, String)>>,
    /// Stack of local variable scopes (for shell functions).
    /// Each entry maps variable names to their previous value (None = was unset).
    local_scopes: Vec<HashMap<String, Option<String>>>,
    fds: HashMap<Fd, FdKind>,
    next_fd: Fd,
    pub bg_counter: u32,
    /// Offset within current arg for getopts combined flags (e.g. -abc).
    pub optoff: i32,
    /// Set of readonly variable names.
    pub readonly_vars: Arc<std::collections::HashSet<String>>,
    /// Shell aliases.
    pub aliases: Arc<HashMap<String, String>>,
    /// Command hash table (name → full path).
    pub hash_table: Arc<HashMap<String, String>>,
    /// Optional stderr channel for sandboxed error output.
    err_tx: Option<mpsc::Sender<Bytes>>,
    /// Current recursion depth (incremented on function calls, subshells, eval, source, command substitution).
    pub depth: u32,
    /// Maximum allowed recursion depth (0 = unlimited).
    pub max_depth: u32,
    /// Deadline for script execution (None = no timeout).
    #[cfg(not(target_arch = "wasm32"))]
    pub deadline: Option<tokio::time::Instant>,
    #[cfg(target_arch = "wasm32")]
    pub deadline: Option<std::time::Instant>,
    /// Maximum bytes for any single string accumulation (0 = unlimited).
    pub max_output: usize,
    /// Maximum number of open file descriptors (0 = unlimited).
    pub max_fds: usize,
    /// Maximum number of background jobs (0 = unlimited).
    pub max_bg_jobs: usize,
    /// Maximum number of pipeline stages (0 = unlimited).
    pub max_pipeline: usize,
    /// Maximum input size for the parser in bytes (0 = unlimited).
    pub max_input: usize,
    /// When true, pipelines capture stdout instead of copying to real stdout.
    pub capture: bool,
    /// Captured stdout output (populated when capture=true).
    pub captured_output: String,
    /// Captured stderr output (populated when capture=true).
    pub captured_stderr: String,
    /// Trap handlers (signal name → command string).
    pub traps: HashMap<String, String>,
    /// File creation mask.
    pub umask: u32,
}

/// Resource limits that can be extracted from a configured Process
/// and applied to fresh processes (e.g., MCP per-request processes).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ProcessLimits {
    pub max_depth: u32,
    pub max_output: usize,
    pub max_fds: usize,
    pub max_bg_jobs: usize,
    pub max_pipeline: usize,
    pub max_input: usize,
    #[serde(default, deserialize_with = "deserialize_timeout")]
    pub timeout: Option<std::time::Duration>,
}

fn deserialize_timeout<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<std::time::Duration>, D::Error> {
    let secs: Option<u64> = Option::deserialize(d)?;
    Ok(secs.map(std::time::Duration::from_secs))
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_output: 1024 * 1024,
            max_fds: 128,
            max_bg_jobs: 8,
            max_pipeline: 16,
            max_input: 1024 * 1024,
            timeout: Some(std::time::Duration::from_secs(30)),
        }
    }
}

impl Process {
    /// Extract the configured resource limits from this process.
    pub fn limits(&self) -> ProcessLimits {
        ProcessLimits {
            max_depth: self.max_depth,
            max_output: self.max_output,
            max_fds: self.max_fds,
            max_bg_jobs: self.max_bg_jobs,
            max_pipeline: self.max_pipeline,
            max_input: self.max_input,
            timeout: {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.deadline
                        .map(|dl| dl.duration_since(tokio::time::Instant::now()))
                }
                #[cfg(target_arch = "wasm32")]
                {
                    self.deadline
                        .and_then(|dl| dl.checked_duration_since(std::time::Instant::now()))
                }
            },
        }
    }

    /// Apply resource limits to this process.
    pub fn apply_limits(&mut self, limits: &ProcessLimits) {
        self.max_depth = limits.max_depth;
        self.max_output = limits.max_output;
        self.max_fds = limits.max_fds;
        self.max_bg_jobs = limits.max_bg_jobs;
        self.max_pipeline = limits.max_pipeline;
        self.max_input = limits.max_input;
        if let Some(dur) = limits.timeout {
            #[cfg(not(target_arch = "wasm32"))]
            {
                self.deadline = Some(tokio::time::Instant::now() + dur);
            }
            #[cfg(target_arch = "wasm32")]
            {
                self.deadline = Some(std::time::Instant::now() + dur);
            }
        }
    }
    pub fn new(cwd: PathBuf, env: HashMap<String, String>) -> Self {
        Self {
            pid: NEXT_PID.fetch_add(1, Ordering::Relaxed),
            cwd,
            env: Arc::new(env),
            functions: Arc::new(HashMap::new()),
            last_exit: 0,
            arg0: "lash".into(),
            args: Vec::new(),
            opt_errexit: false,
            opt_nounset: false,
            opt_xtrace: false,
            nounset_error: false,
            last_bg_pid: None,
            bg_jobs: Vec::new(),
            local_scopes: Vec::new(),
            fds: HashMap::new(),
            next_fd: 3,
            bg_counter: 0,
            optoff: -1,
            readonly_vars: Arc::new(std::collections::HashSet::new()),
            aliases: Arc::new(HashMap::new()),
            hash_table: Arc::new(HashMap::new()),
            err_tx: None,
            depth: 0,
            max_depth: 0,
            deadline: None,
            max_output: 0,
            max_fds: 0,
            max_bg_jobs: 0,
            max_pipeline: 0,
            max_input: 0,
            capture: false,
            captured_output: String::new(),
            captured_stderr: String::new(),
            traps: HashMap::new(),
            umask: 0o022,
        }
    }

    /// Create a placeholder empty process (used for temporary swaps).
    pub fn empty() -> Self {
        Self {
            pid: 0,
            cwd: PathBuf::new(),
            env: Arc::new(HashMap::new()),
            functions: Arc::new(HashMap::new()),
            last_exit: 0,
            arg0: String::new(),
            args: Vec::new(),
            opt_errexit: false,
            opt_nounset: false,
            opt_xtrace: false,
            nounset_error: false,
            last_bg_pid: None,
            bg_jobs: Vec::new(),
            local_scopes: Vec::new(),
            fds: HashMap::new(),
            next_fd: 3,
            bg_counter: 0,
            optoff: -1,
            readonly_vars: Arc::new(std::collections::HashSet::new()),
            aliases: Arc::new(HashMap::new()),
            hash_table: Arc::new(HashMap::new()),
            err_tx: None,
            depth: 0,
            max_depth: 0,
            deadline: None,
            max_output: 0,
            max_fds: 0,
            max_bg_jobs: 0,
            max_pipeline: 0,
            max_input: 0,
            capture: false,
            captured_output: String::new(),
            captured_stderr: String::new(),
            traps: HashMap::new(),
            umask: 0o022,
        }
    }

    pub fn alloc_fd(&mut self, kind: FdKind) -> io::Result<Fd> {
        if self.max_fds > 0 && self.fds.len() >= self.max_fds {
            return Err(io::Error::other("too many open file descriptors"));
        }
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(fd, kind);
        Ok(fd)
    }

    /// Fork this process — child inherits cwd and env but gets empty fd table.
    pub fn fork(&self) -> Process {
        Process {
            pid: self.pid,
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            functions: self.functions.clone(),
            last_exit: self.last_exit,
            arg0: self.arg0.clone(),
            args: self.args.clone(),
            opt_errexit: self.opt_errexit,
            opt_nounset: self.opt_nounset,
            opt_xtrace: self.opt_xtrace,
            nounset_error: false,
            last_bg_pid: None,
            bg_jobs: Vec::new(),
            local_scopes: Vec::new(),
            fds: HashMap::new(),
            next_fd: 3,
            bg_counter: 0,
            optoff: self.optoff,
            readonly_vars: self.readonly_vars.clone(),
            aliases: self.aliases.clone(),
            hash_table: self.hash_table.clone(),
            err_tx: self.err_tx.clone(),
            depth: self.depth,
            max_depth: self.max_depth,
            deadline: self.deadline,
            max_output: self.max_output,
            max_fds: self.max_fds,
            max_bg_jobs: self.max_bg_jobs,
            max_pipeline: self.max_pipeline,
            max_input: self.max_input,
            capture: self.capture,
            captured_output: String::new(),
            captured_stderr: String::new(),
            traps: self.traps.clone(),
            umask: self.umask,
        }
    }

    /// Install a pipe: writer on `self[writer_fd]`, reader on returned Process-less FdReader.
    /// Use `set_channel_writer` / `set_channel_reader` for cross-process pipes.
    pub fn set_channel_reader(&mut self, fd: Fd, rx: mpsc::Receiver<Bytes>) {
        self.fds.insert(
            fd,
            FdKind::ChannelReader {
                rx,
                buf: Vec::new(),
            },
        );
    }

    pub fn set_channel_writer(&mut self, fd: Fd, tx: mpsc::Sender<Bytes>) {
        self.fds.insert(
            fd,
            FdKind::ChannelWriter {
                tx,
                error_flag: None,
            },
        );
    }

    /// Remove an fd from this process and install it in another.
    /// Used to pass stdin across fork boundaries (e.g. CompoundPipeline).
    pub fn transfer_fd(&mut self, fd: Fd, target: &mut Process) {
        if let Some(kind) = self.fds.remove(&fd) {
            target.fds.insert(fd, kind);
        }
    }

    pub fn dup2(&mut self, from: Fd, to: Fd) -> io::Result<()> {
        let kind = self
            .fds
            .remove(&from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("bad fd {from}")))?;
        self.fds.insert(to, kind);
        Ok(())
    }

    /// Duplicate an fd (keeping the source open) by cloning its channel.
    pub async fn dup_fd(&mut self, src: Fd, dst: Fd) -> io::Result<()> {
        let kind = self
            .fds
            .get(&src)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("bad fd {src}")))?
            .try_clone()
            .await?;
        self.fds.insert(dst, kind);
        Ok(())
    }

    /// Take the reader half out of an fd, removing it from the table.
    pub fn take_reader(&mut self, fd: Fd) -> io::Result<FdReader> {
        match self.fds.remove(&fd) {
            Some(kind) => Ok(FdReader { kind, done: false }),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("bad fd {fd}"),
            )),
        }
    }

    /// Take the writer half out of an fd, removing it from the table.
    pub fn take_writer(&mut self, fd: Fd) -> io::Result<FdWriter> {
        match self.fds.remove(&fd) {
            Some(kind) => Ok(FdWriter { kind }),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("bad fd {fd}"),
            )),
        }
    }

    pub fn close(&mut self, fd: Fd) {
        self.fds.remove(&fd);
    }

    /// Check whether an fd exists in this process.
    pub fn has_fd(&self, fd: Fd) -> bool {
        self.fds.contains_key(&fd)
    }

    /// Restore a previously taken fd (e.g. after `take_reader`).
    pub fn restore_fd(&mut self, fd: Fd, kind: FdKind) {
        self.fds.insert(fd, kind);
    }

    /// Set the stderr channel for sandboxed error output.
    pub fn set_err_tx(&mut self, tx: mpsc::Sender<Bytes>) {
        self.err_tx = Some(tx);
    }

    /// Check execution limits (deadline and recursion depth).
    /// Returns an error message if a limit is exceeded.
    pub fn check_limits(&self) -> Option<&'static str> {
        if let Some(dl) = self.deadline {
            #[cfg(not(target_arch = "wasm32"))]
            let expired = tokio::time::Instant::now() >= dl;
            #[cfg(target_arch = "wasm32")]
            let expired = std::time::Instant::now() >= dl;
            if expired {
                return Some("strands-shell: execution timeout exceeded");
            }
        }
        if self.max_depth > 0 && self.depth >= self.max_depth {
            return Some("strands-shell: maximum recursion depth exceeded");
        }
        None
    }

    /// Clear the stderr channel (allows the channel to close).
    pub fn clear_err_tx(&mut self) {
        self.err_tx = None;
    }

    /// Write an error message to the process stderr channel, or real stderr as fallback.
    pub fn err_msg(&mut self, msg: &str) {
        if let Some(tx) = &self.err_tx {
            let _ = tx.try_send(Bytes::from(format!("{msg}\n")));
        } else if self.capture {
            self.captured_stderr.push_str(msg);
            self.captured_stderr.push('\n');
        } else {
            eprintln!("{msg}");
        }
    }

    /// Write a message to stdout (fd 1 channel if available, else real stdout).
    pub fn out_msg(&mut self, msg: &str) {
        if let Some(FdKind::ChannelWriter { tx, .. }) = self.fds.get(&STDOUT) {
            let _ = tx.try_send(Bytes::from(format!("{msg}\n")));
        } else if self.capture {
            if self.max_output > 0 && self.captured_output.len() + msg.len() > self.max_output {
                self.captured_stderr
                    .push_str("strands-shell: output size limit exceeded\n");
                self.last_exit = 1;
                return;
            }
            self.captured_output.push_str(msg);
            self.captured_output.push('\n');
        } else {
            println!("{msg}");
        }
    }

    /// Set an environment variable (COW — clones map on first write if shared).
    /// Returns false if the variable is readonly.
    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) -> bool {
        let key = key.into();
        if self.readonly_vars.contains(&key) {
            self.err_msg(&format!("strands-shell: {key}: readonly variable"));
            return false;
        }
        Arc::make_mut(&mut self.env).insert(key, value.into());
        true
    }

    /// Remove an environment variable. Returns false if readonly.
    pub fn unset_env(&mut self, key: &str) -> bool {
        if self.readonly_vars.contains(key) {
            self.err_msg(&format!("strands-shell: {key}: readonly variable"));
            return false;
        }
        Arc::make_mut(&mut self.env).remove(key);
        true
    }

    /// Mark a variable as readonly.
    pub fn mark_readonly(&mut self, key: impl Into<String>) {
        Arc::make_mut(&mut self.readonly_vars).insert(key.into());
    }

    /// Define a shell function.
    pub fn set_function(&mut self, name: impl Into<String>, body: crate::parser::CommandLine) {
        Arc::make_mut(&mut self.functions).insert(name.into(), body);
    }

    /// Look up a shell function.
    pub fn get_function(&self, name: &str) -> Option<&crate::parser::CommandLine> {
        self.functions.get(name)
    }

    /// Remove a shell function.
    pub fn unset_function(&mut self, name: &str) {
        Arc::make_mut(&mut self.functions).remove(name);
    }

    /// Set a shell alias.
    pub fn set_alias(&mut self, name: impl Into<String>, value: impl Into<String>) {
        Arc::make_mut(&mut self.aliases).insert(name.into(), value.into());
    }

    /// Remove a shell alias.
    pub fn unset_alias(&mut self, name: &str) -> bool {
        let map = Arc::make_mut(&mut self.aliases);
        map.remove(name).is_some()
    }

    /// Remove all aliases.
    pub fn clear_aliases(&mut self) {
        Arc::make_mut(&mut self.aliases).clear();
    }

    /// Look up an environment variable.
    pub fn get_env(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(|s| s.as_str())
    }

    /// Push a new local variable scope (called when entering a function).
    pub fn push_local_scope(&mut self) {
        self.local_scopes.push(HashMap::new());
    }

    /// Pop the top local scope, restoring all localized variables.
    pub fn pop_local_scope(&mut self) {
        if let Some(scope) = self.local_scopes.pop() {
            for (name, prev) in scope {
                match prev {
                    Some(val) => {
                        self.set_env(&name, &val);
                    }
                    None => {
                        self.unset_env(&name);
                    }
                }
            }
        }
    }

    /// Declare a variable as local: save its current value in the top scope,
    /// then set the new value. If already saved in this scope, just set.
    pub fn set_local(&mut self, name: &str, value: &str) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope
                .entry(name.to_string())
                .or_insert_with(|| self.env.get(name).cloned());
        }
        self.set_env(name, value);
    }

    /// Declare a variable as local without assigning (preserve or set empty).
    pub fn declare_local(&mut self, name: &str) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope
                .entry(name.to_string())
                .or_insert_with(|| self.env.get(name).cloned());
        }
    }
}

/// Read from an async reader into a String, enforcing an optional size limit.
/// Returns Err if the limit is exceeded.
pub async fn read_to_string_limited<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<String> {
    let buf = read_to_end_limited(reader, limit).await?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Read from an async reader into a byte vector, enforcing an optional size
/// limit. Returns Err if the limit is exceeded. A `limit` of 0 means no cap.
pub async fn read_to_end_limited<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Vec<u8>> {
    if limit == 0 {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(reader, &mut buf).await?;
        return Ok(buf);
    }
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let n = tokio::io::AsyncReadExt::read(reader, &mut tmp).await?;
        if n == 0 {
            break;
        }
        if buf.len() + n > limit {
            return Err(io::Error::other("output size limit exceeded"));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Ok(buf)
}

/// Standard base64 encoder (RFC 4648). Self-contained — no external dep.
pub fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Create a bounded channel pair for use as a pipe.
pub fn pipe(buffer: usize) -> (mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
    mpsc::channel(buffer)
}

/// Owned async reader extracted from a Process fd.
pub struct FdReader {
    kind: FdKind,
    done: bool,
}

impl FdReader {
    /// Create an FdReader directly from a channel receiver.
    pub fn from_receiver(rx: mpsc::Receiver<Bytes>) -> Self {
        Self {
            kind: FdKind::ChannelReader {
                rx,
                buf: Vec::new(),
            },
            done: false,
        }
    }

    /// Consume this reader and return the underlying FdKind.
    pub fn into_fd_kind(self) -> FdKind {
        self.kind
    }
}

impl AsyncRead for FdReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(Ok(()));
        }
        match &mut this.kind {
            FdKind::ChannelReader { rx, buf: remainder } => {
                if !remainder.is_empty() {
                    let n = remainder.len().min(buf.remaining());
                    buf.put_slice(&remainder[..n]);
                    remainder.drain(..n);
                    return Poll::Ready(Ok(()));
                }
                match rx.poll_recv(cx) {
                    Poll::Ready(Some(bytes)) => {
                        let n = bytes.len().min(buf.remaining());
                        buf.put_slice(&bytes[..n]);
                        if n < bytes.len() {
                            remainder.extend_from_slice(&bytes[n..]);
                        }
                        Poll::Ready(Ok(()))
                    }
                    Poll::Ready(None) => {
                        this.done = true;
                        Poll::Ready(Ok(()))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            FdKind::File(f) => Pin::new(f).poll_read(cx, buf),
            FdKind::ChannelWriter { .. } => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fd not readable",
            ))),
        }
    }
}

/// Owned async writer extracted from a Process fd.
pub struct FdWriter {
    kind: FdKind,
}

impl AsyncWrite for FdWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match &mut this.kind {
            FdKind::ChannelWriter { tx, error_flag } => {
                if let Some(flag) = error_flag
                    && flag.load(std::sync::atomic::Ordering::Relaxed)
                {
                    return Poll::Ready(Err(io::Error::other("file size limit exceeded")));
                }
                let bytes = Bytes::copy_from_slice(buf);
                let len = bytes.len();
                match tx.try_send(bytes) {
                    Ok(()) => Poll::Ready(Ok(len)),
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Channel full — we need to wait. Store bytes and poll again.
                        // For simplicity, use a waker-based approach via try_send retry.
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "pipe closed",
                    ))),
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            FdKind::File(f) => Pin::new(f).poll_write(cx, buf),
            FdKind::ChannelReader { .. } => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fd not writable",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().kind {
            #[cfg(not(target_arch = "wasm32"))]
            FdKind::File(f) => Pin::new(f).poll_flush(cx),
            _ => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().kind {
            #[cfg(not(target_arch = "wasm32"))]
            FdKind::File(f) => Pin::new(f).poll_shutdown(cx),
            _ => Poll::Ready(Ok(())),
        }
    }
}

/// HTTP request passed to [`Kernel::http_request`].
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Allow invalid TLS certificates (curl -k).
    pub insecure: bool,
    /// Maximum response body size in bytes (0 = unlimited).
    pub max_response: usize,
}

/// HTTP response returned by [`Kernel::http_request`].
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// HTTP version string (e.g. "1.1", "2").
    pub version: String,
    /// Canonical reason phrase (e.g. "OK", "Not Found").
    pub reason: String,
}

/// The core kernel abstraction. All methods take &self — the kernel is shared.
#[async_trait]
pub trait Kernel: Send + Sync {
    fn new_process(&self) -> Process;
    async fn open(&self, proc: &mut Process, path: &str, flags: OpenFlags) -> io::Result<Fd>;
    async fn list_dir(&self, proc: &Process, path: &str) -> io::Result<Vec<DirEntry>>;
    async fn change_dir(&self, proc: &mut Process, path: &str) -> io::Result<()>;
    /// Stat a file (follows symlinks). Returns default (exists=false) on error.
    async fn stat(&self, proc: &Process, path: &str) -> FileStat;
    /// Stat a file (does not follow symlinks).
    async fn lstat(&self, proc: &Process, path: &str) -> FileStat;
    /// Check access permissions (ACCESS_R, ACCESS_W, ACCESS_X).
    async fn access(&self, proc: &Process, path: &str, mode: i32) -> bool;
    /// Canonicalize a path (resolve symlinks).
    async fn canonicalize(&self, proc: &Process, path: &str) -> io::Result<PathBuf>;
    /// Check if a path is an executable file.
    async fn is_executable(&self, proc: &Process, path: &str) -> bool;
    /// Expand a glob pattern relative to the process cwd. Returns sorted matches.
    async fn glob(&self, proc: &Process, pattern: &str) -> Vec<String>;
    /// Check if a file descriptor refers to a terminal.
    fn isatty(&self, fd: i32) -> bool;
    /// Remove a file.
    async fn remove_file(&self, proc: &Process, path: &str) -> io::Result<()>;
    /// Remove an empty directory.
    async fn remove_dir(&self, proc: &Process, path: &str) -> io::Result<()>;
    /// Create a directory.
    async fn create_dir(&self, proc: &Process, path: &str) -> io::Result<()>;
    /// Rename (move) a file or directory.
    async fn rename(&self, proc: &Process, from: &str, to: &str) -> io::Result<()>;
    /// Create a symbolic link at `link` pointing to `target`.
    async fn symlink(&self, proc: &Process, target: &str, link: &str) -> io::Result<()>;
    /// Read the target of a symbolic link.
    async fn read_link(&self, proc: &Process, path: &str) -> io::Result<String>;
    /// Set Unix permission mode bits on a path.
    async fn set_permissions(&self, proc: &Process, path: &str, mode: u32) -> io::Result<()>;
    /// Return the current wall-clock time.
    fn now(&self) -> std::time::SystemTime;
    /// Check whether a URL is allowed for network access.
    /// Returns Ok(()) if allowed, Err with a message if blocked.
    fn check_url(&self, _url: &str) -> io::Result<()> {
        Ok(())
    }
    /// Look up credentials for a URL and HTTP method.
    /// Returns a list of HTTP headers to inject.
    fn resolve_credential(&self, _url: &str, _method: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Send an HTTP request. The kernel handles SSRF protection,
    /// credential injection, and the actual network transport.
    async fn http_request(&self, _req: HttpRequest) -> io::Result<HttpResponse> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "HTTP not available",
        ))
    }
}
