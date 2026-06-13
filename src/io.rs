use std::cell::RefCell;
#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use crate::mcp_client::NamedMcpClient;
use crate::os::{Fd, FdReader, FdWriter, Kernel, OpenFlags, Process, STDERR, STDIN, STDOUT};

tokio::task_local! {
    pub static CURRENT_PROCESS: RefCell<Process>;
    pub static CURRENT_KERNEL: Arc<dyn Kernel>;
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static MCP_CLIENTS: RefCell<Option<Rc<Vec<NamedMcpClient>>>> = const { RefCell::new(None) };
}

/// Get the current kernel Arc from the task-local context.
pub fn kernel() -> Arc<dyn Kernel> {
    CURRENT_KERNEL.with(|k| k.clone())
}

/// Get the MCP clients from the thread-local context, if set.
#[cfg(not(target_arch = "wasm32"))]
pub fn mcp_clients() -> Option<Rc<Vec<NamedMcpClient>>> {
    MCP_CLIENTS.with(|c| c.borrow().clone())
}

/// Set the MCP clients in the thread-local context.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_mcp_clients(clients: Rc<Vec<NamedMcpClient>>) {
    MCP_CLIENTS.with(|c| *c.borrow_mut() = Some(clients));
}

/// Take stdout (fd 1) from the current process.
pub fn stdout() -> std::io::Result<FdWriter> {
    CURRENT_PROCESS.with(|p| p.borrow_mut().take_writer(STDOUT))
}

/// Take stdin (fd 0) from the current process.
pub fn stdin() -> std::io::Result<FdReader> {
    CURRENT_PROCESS.with(|p| p.borrow_mut().take_reader(STDIN))
}

/// Take stderr (fd 2) from the current process.
pub fn stderr() -> std::io::Result<FdWriter> {
    CURRENT_PROCESS.with(|p| p.borrow_mut().take_writer(STDERR))
}

/// Take a reader for an arbitrary fd from the current process.
pub fn take_reader(fd: Fd) -> std::io::Result<FdReader> {
    CURRENT_PROCESS.with(|p| p.borrow_mut().take_reader(fd))
}

/// Take a writer for an arbitrary fd from the current process.
pub fn take_writer(fd: Fd) -> std::io::Result<FdWriter> {
    CURRENT_PROCESS.with(|p| p.borrow_mut().take_writer(fd))
}

/// Access the current process in a closure (for cwd, etc).
pub fn with_process<F, R>(f: F) -> R
where
    F: FnOnce(&mut Process) -> R,
{
    CURRENT_PROCESS.with(|p| f(&mut p.borrow_mut()))
}

/// Open a file via the kernel, using the current process for path resolution.
pub async fn open(os: &dyn Kernel, path: &str, flags: OpenFlags) -> std::io::Result<Fd> {
    // Borrow process only for the synchronous parts inside open.
    // The Kernel::open takes &mut Process, so we temporarily take it out.
    let mut proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.open(&mut proc, path, flags).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Change directory via the kernel on the current process.
pub async fn change_dir(os: &dyn Kernel, path: &str) -> std::io::Result<()> {
    let mut proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.change_dir(&mut proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// List directory via the kernel using the current process for path resolution.
pub async fn list_dir(os: &dyn Kernel, path: &str) -> std::io::Result<Vec<crate::os::DirEntry>> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.list_dir(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Lstat a file (don't follow symlinks) via the kernel.
pub async fn lstat(os: &dyn Kernel, path: &str) -> crate::os::FileStat {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.lstat(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Stat a file via the kernel using the current process for path resolution.
pub async fn stat(os: &dyn Kernel, path: &str) -> crate::os::FileStat {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.stat(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Remove a file via the kernel.
pub async fn remove_file(os: &dyn Kernel, path: &str) -> std::io::Result<()> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.remove_file(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Remove an empty directory via the kernel.
pub async fn remove_dir(os: &dyn Kernel, path: &str) -> std::io::Result<()> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.remove_dir(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Create a directory via the kernel.
pub async fn create_dir(os: &dyn Kernel, path: &str) -> std::io::Result<()> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.create_dir(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Rename (move) a file or directory via the kernel.
pub async fn rename(os: &dyn Kernel, from: &str, to: &str) -> std::io::Result<()> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.rename(&proc, from, to).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Create a symbolic link via the kernel.
pub async fn symlink(os: &dyn Kernel, target: &str, link: &str) -> std::io::Result<()> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.symlink(&proc, target, link).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Read a symbolic link target via the kernel.
pub async fn read_link(os: &dyn Kernel, path: &str) -> std::io::Result<String> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.read_link(&proc, path).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

/// Set permissions on a path via the kernel.
pub async fn set_permissions(os: &dyn Kernel, path: &str, mode: u32) -> std::io::Result<()> {
    let proc = CURRENT_PROCESS.with(|p| p.replace(Process::empty()));
    let result = os.set_permissions(&proc, path, mode).await;
    CURRENT_PROCESS.with(|p| p.replace(proc));
    result
}

#[macro_export]
macro_rules! wprint {
    ($w:expr, $($arg:tt)*) => {
        $w.write_all(format!($($arg)*).as_bytes()).await
    };
}

#[macro_export]
macro_rules! wprintln {
    ($w:expr) => {
        $w.write_all(b"\n").await
    };
    ($w:expr, $($arg:tt)*) => {
        $w.write_all(format!("{}\n", format_args!($($arg)*)).as_bytes()).await
    };
}
