//! C-ABI reactor exports for embedding the shell in WASI preview1 hosts
//! (`wasm-api` feature, built as a `wasm32-wasip1` cdylib).
//!
//! The ABI is experimental. SDKs call `shell_abi_version` first and refuse to
//! run against a version they were not written for; any incompatible change to
//! an export's signature, the result buffer layout or the call semantics bumps
//! [`ABI_VERSION`].
//!
//! The instance stays alive across calls: each handle returned by `shell_new`
//! owns a [`Shell`] whose state (environment, working directory, VFS) persists
//! between `shell_run` calls, mirroring the Python and Node bindings. Pointers
//! and lengths are `u32` offsets into linear memory; a `(ptr, len)` pair is
//! packed into a `u64` as `ptr << 32 | len`. Buffers handed out by `shell_run`
//! belong to the caller until released with `shell_buf_free`; shells
//! themselves live until `shell_free`. Failures set a
//! message readable through `shell_last_error`.
//!
//! `wasm32-wasip1` aborts on panic, so a panic traps the instance instead of
//! unwinding across the boundary. Calls are not reentrant: a shell's entry is
//! taken out of the table while its script runs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::slice;

use tokio::task::LocalSet;

use crate::Shell;

/// Version of the exported C ABI, returned by `shell_abi_version`.
pub const ABI_VERSION: u32 = 1;

struct Entry {
    shell: Shell,
    runtime: tokio::runtime::Runtime,
}

struct State {
    next_handle: u32,
    shells: HashMap<u32, Entry>,
    last_error: Vec<u8>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        next_handle: 1,
        shells: HashMap::new(),
        last_error: Vec::new(),
    });
}

unsafe extern "C" {
    fn __wasm_call_ctors();
}

/// Reactor entry point: runs static constructors once. Hosts call it exactly
/// once after instantiation; exporting it keeps wasm-ld from wrapping every
/// export with command-style constructor and destructor calls.
#[unsafe(no_mangle)]
pub extern "C" fn _initialize() {
    unsafe { __wasm_call_ctors() }
}

fn pack(ptr: *const u8, len: usize) -> u64 {
    ((ptr as usize as u64) << 32) | len as u64
}

fn leak(bytes: Vec<u8>) -> u64 {
    let len = bytes.len();
    pack(Box::into_raw(bytes.into_boxed_slice()) as *const u8, len)
}

fn set_error(msg: impl ToString) {
    STATE.with(|s| s.borrow_mut().last_error = msg.to_string().into_bytes());
}

/// The ABI version this module implements; see [`ABI_VERSION`].
#[unsafe(no_mangle)]
pub extern "C" fn shell_abi_version() -> u32 {
    ABI_VERSION
}

/// Allocate a `len`-byte buffer in linear memory for the host to write into.
#[unsafe(no_mangle)]
pub extern "C" fn shell_buf_alloc(len: u32) -> *mut u8 {
    Box::into_raw(vec![0u8; len as usize].into_boxed_slice()) as *mut u8
}

/// Release a buffer obtained from `shell_buf_alloc` or `shell_run`.
///
/// # Safety
///
/// `ptr` and `len` must describe exactly one buffer previously returned by
/// this module and not yet released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shell_buf_free(ptr: *mut u8, len: u32) {
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len as usize)) });
}

/// Create a shell from a TOML configuration (same schema as `--config`).
///
/// Returns a non-zero handle, or `0` with the reason in `shell_last_error`.
///
/// # Safety
///
/// `cfg_ptr` must point to `cfg_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shell_new(cfg_ptr: *const u8, cfg_len: u32) -> u32 {
    let cfg = unsafe { slice::from_raw_parts(cfg_ptr, cfg_len as usize) };
    match new_shell(cfg) {
        Ok(shell) => STATE.with(|s| {
            let mut s = s.borrow_mut();
            let handle = s.next_handle;
            s.next_handle += 1;
            s.shells.insert(handle, shell);
            handle
        }),
        Err(e) => {
            set_error(e);
            0
        }
    }
}

fn new_shell(cfg: &[u8]) -> io::Result<Entry> {
    let cfg =
        std::str::from_utf8(cfg).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let shell = Shell::builder().config_toml(cfg)?.build()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(Entry { shell, runtime })
}

/// Run a script on the shell behind `handle`.
///
/// Returns a packed `(ptr, len)` of a buffer laid out as little-endian
/// `i32 status`, `u32 stdout_len`, `u32 stderr_len`, followed by the stdout
/// and stderr bytes. Returns `0` for an unknown handle or a script that is not
/// UTF-8, with the reason in `shell_last_error`.
///
/// # Safety
///
/// `script_ptr` must point to `script_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shell_run(handle: u32, script_ptr: *const u8, script_len: u32) -> u64 {
    let script = unsafe { slice::from_raw_parts(script_ptr, script_len as usize) };
    let Ok(script) = std::str::from_utf8(script) else {
        set_error("script is not valid UTF-8");
        return 0;
    };
    let Some(mut entry) = STATE.with(|s| s.borrow_mut().shells.remove(&handle)) else {
        set_error("invalid shell handle");
        return 0;
    };
    let local = LocalSet::new();
    let out = entry
        .runtime
        .block_on(local.run_until(entry.shell.run(script)));
    STATE.with(|s| s.borrow_mut().shells.insert(handle, entry));
    let mut buf = Vec::with_capacity(12 + out.stdout.len() + out.stderr.len());
    buf.extend_from_slice(&out.status.to_le_bytes());
    buf.extend_from_slice(&(out.stdout.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(out.stderr.len() as u32).to_le_bytes());
    buf.extend_from_slice(out.stdout.as_bytes());
    buf.extend_from_slice(out.stderr.as_bytes());
    leak(buf)
}

/// Packed `(ptr, len)` of the last error message, valid until the next call
/// into this module. The buffer is owned by the module.
#[unsafe(no_mangle)]
pub extern "C" fn shell_last_error() -> u64 {
    STATE.with(|s| {
        let s = s.borrow();
        pack(s.last_error.as_ptr(), s.last_error.len())
    })
}

/// Drop the shell behind `handle`. Unknown handles are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn shell_free(handle: u32) {
    STATE.with(|s| {
        s.borrow_mut().shells.remove(&handle);
    });
}
