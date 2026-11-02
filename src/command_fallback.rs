//! Optional handler for commands that Shell does not implement.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::os::{FdReader, FdWriter, Process, STDERR, STDIN, STDOUT};

#[cfg(not(target_arch = "wasm32"))]
type Instant = tokio::time::Instant;
#[cfg(target_arch = "wasm32")]
type Instant = std::time::Instant;

/// Runs a command that no builtin, function, registered command, or
/// virtual executable provides.
///
/// A [`Kernel`](crate::os::Kernel) exposes a handler through
/// [`Kernel::command_fallback`](crate::os::Kernel::command_fallback).
/// Pipeline stages can call `run` concurrently.
///
/// | Result | Shell behavior |
/// | --- | --- |
/// | `Ok(Some(status))` | Uses `status`, including 127, and writes no diagnostic. |
/// | `Ok(None)` | Writes `command not found` to the command's stderr and returns 127. |
/// | `Err(error)` | Writes `error` to the command's stderr and returns 1. |
///
/// Decline with `Ok(None)` before using the streams or starting external
/// work. Shell drops the future when the execution deadline expires or the
/// foreground execution is dropped; the handler must then stop and reap any
/// external work it started. After a dropped execution, the drop happens the
/// next time the runtime polls the shell's tasks. On `wasm32`, Shell checks
/// the deadline only before it calls the handler, so the handler must enforce
/// [`CommandContext::deadline`] itself.
#[async_trait]
pub trait CommandFallback: Send + Sync {
    async fn run(&self, ctx: &mut CommandContext) -> io::Result<Option<i32>>;
}

/// The command's redirected stdin.
pub struct CommandInput {
    inner: Option<FdReader>,
}

impl CommandInput {
    /// Release the input. Later reads return end of file.
    pub fn close(&mut self) {
        self.inner.take();
    }
}

impl AsyncRead for CommandInput {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut().inner.as_mut() {
            Some(reader) => Pin::new(reader).poll_read(cx, buf),
            None => Poll::Ready(Ok(())),
        }
    }
}

/// The command's redirected stdout or stderr.
///
/// Shutdown releases the writer, so a reader sees end of file before the
/// handler returns, unless another descriptor such as `2>&1` still refers to
/// the same pipe. Later writes fail with [`io::ErrorKind::BrokenPipe`].
pub struct CommandOutput {
    inner: Option<FdWriter>,
}

impl AsyncWrite for CommandOutput {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut().inner.as_mut() {
            Some(writer) => Pin::new(writer).poll_write(cx, buf),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "command output is closed",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut().inner.as_mut() {
            Some(writer) => Pin::new(writer).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let Some(writer) = this.inner.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        let result = std::task::ready!(Pin::new(writer).poll_shutdown(cx));
        this.inner = None;
        Poll::Ready(result)
    }
}

/// One invocation of a command that Shell does not implement.
pub struct CommandContext {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: Arc<HashMap<String, String>>,
    deadline: Option<Instant>,
    stdin: Option<CommandInput>,
    stdout: Option<CommandOutput>,
    stderr: Option<CommandOutput>,
}

impl CommandContext {
    pub(crate) fn new(proc: &mut Process, program: String, args: Vec<String>) -> Self {
        Self {
            program,
            args,
            cwd: proc.cwd.clone(),
            env: proc.env.clone(),
            deadline: proc.deadline,
            stdin: proc.take_reader(STDIN).ok().map(|reader| CommandInput {
                inner: Some(reader),
            }),
            stdout: proc.take_writer(STDOUT).ok().map(|writer| CommandOutput {
                inner: Some(writer),
            }),
            stderr: proc.take_writer(STDERR).ok().map(|writer| CommandOutput {
                inner: Some(writer),
            }),
        }
    }

    /// The program name after expansion and multicall or shebang resolution.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The expanded arguments, without the program name.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The virtual working directory.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The Shell variables, including command-local assignments, unexported
    /// variables, and function locals.
    pub fn env(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// The absolute execution deadline, if the shell has a timeout.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }

    /// The absolute execution deadline, if the shell has a timeout.
    #[cfg(target_arch = "wasm32")]
    pub fn deadline(&self) -> Option<std::time::Instant> {
        self.deadline
    }

    /// The stdin, stdout, and stderr streams after redirection.
    ///
    /// A stream is `None` when its descriptor is closed or unavailable.
    pub fn stdio(
        &mut self,
    ) -> (
        Option<&mut CommandInput>,
        Option<&mut CommandOutput>,
        Option<&mut CommandOutput>,
    ) {
        (
            self.stdin.as_mut(),
            self.stdout.as_mut(),
            self.stderr.as_mut(),
        )
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "execution timeout exceeded")
}

async fn bounded(
    handler: &dyn CommandFallback,
    context: &mut CommandContext,
) -> io::Result<Option<i32>> {
    let Some(deadline) = context.deadline else {
        return handler.run(context).await;
    };
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::timeout_at(deadline, handler.run(context))
            .await
            .unwrap_or_else(|_| Err(timed_out()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        handler.run(context).await
    }
}

/// Run `handler` for one pipeline stage and map its result to a status.
pub(crate) async fn run(handler: &dyn CommandFallback, mut context: CommandContext) -> i32 {
    let (status, diagnostic) = match bounded(handler, &mut context).await {
        Ok(Some(status)) => return status,
        Ok(None) => (127, "command not found".to_string()),
        Err(error) => (1, error.to_string()),
    };
    if let Some(stderr) = &mut context.stderr {
        let message = format!("strands-shell: {}: {diagnostic}\n", context.program);
        let _ = stderr.write_all(message.as_bytes()).await;
    }
    status
}
