//! Python bindings for Strands Shell shell.
//!
//! This is the low-level native extension (`strands_shell._native`). The
//! customer-facing surface — the config-driven `Shell`, the `Bind` / `Cred` /
//! `Limits` dataclasses, and the typed `ShellError` exception hierarchy — lives
//! in the pure-Python wrapper `strands_shell/__init__.py`, which translates config
//! objects into the builder calls exposed here and maps `NativeShellError`
//! (carrying a `.kind`) onto the typed exceptions.

use std::time::Duration;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::shell::FileOpErrorKind;

pyo3::create_exception!(
    strands_shell,
    NativeShellError,
    pyo3::exceptions::PyException,
    "Low-level file-op error raised by the native extension. Carries `kind` \
     (\"not_found\" | \"permission_denied\" | \"too_large\" | \"other\"), \
     `path`, and `message`. The Python wrapper maps it onto the typed \
     `ShellError` hierarchy."
);

/// Build a `NativeShellError` from a file-op `io::Error`, classifying it and
/// attaching `kind` / `path` / `message` attributes for the wrapper.
fn native_file_error(py: Python<'_>, path: &str, err: &std::io::Error) -> PyErr {
    let kind = match FileOpErrorKind::classify(err) {
        FileOpErrorKind::NotFound => "not_found",
        FileOpErrorKind::PermissionDenied => "permission_denied",
        FileOpErrorKind::TooLarge => "too_large",
        FileOpErrorKind::Other => "other",
    };
    let message = err.to_string();
    let pyerr = NativeShellError::new_err(message.clone());
    // Attach structured attributes so the wrapper doesn't have to parse the
    // message string. Best-effort: if setattr fails we still raise the error.
    let value = pyerr.value(py);
    let _ = value.setattr("kind", kind);
    let _ = value.setattr("path", path);
    let _ = value.setattr("message", message);
    pyerr
}

/// Output from a shell command execution.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct Output {
    #[pyo3(get)]
    pub status: i32,
    #[pyo3(get)]
    pub stdout: String,
    #[pyo3(get)]
    pub stderr: String,
}

#[pymethods]
impl Output {
    fn __repr__(&self) -> String {
        format!(
            "Output(status={}, stdout={:?}, stderr={:?})",
            self.status, self.stdout, self.stderr
        )
    }
}

/// Metadata about a file or directory in the VFS.
///
/// Mirrors the `FileInfo` dataclass from the Strands `Sandbox` ABC so the
/// adapter can convert by attribute copy.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct FileInfo {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub is_dir: Option<bool>,
    #[pyo3(get)]
    pub size: Option<u64>,
}

#[pymethods]
impl FileInfo {
    fn __repr__(&self) -> String {
        let is_dir = match self.is_dir {
            Some(true) => "True",
            Some(false) => "False",
            None => "None",
        };
        let size = match self.size {
            Some(n) => n.to_string(),
            None => "None".to_string(),
        };
        format!(
            "FileInfo(name={:?}, is_dir={}, size={})",
            self.name, is_dir, size
        )
    }
}

/// Builder for configuring a Shell.
#[pyclass]
pub struct ShellBuilder {
    inner: Option<crate::shell::ShellBuilder>,
}

/// Apply a transformation to the inner builder and return the same Python
/// reference, so methods can be chained: `builder.bind(...).timeout(...)`.
fn chain<'py>(
    mut slf: PyRefMut<'py, ShellBuilder>,
    f: impl FnOnce(crate::shell::ShellBuilder) -> crate::shell::ShellBuilder,
) -> PyResult<PyRefMut<'py, ShellBuilder>> {
    let b = slf
        .inner
        .take()
        .ok_or_else(|| PyRuntimeError::new_err("builder consumed"))?;
    slf.inner = Some(f(b));
    Ok(slf)
}

#[pymethods]
impl ShellBuilder {
    #[new]
    fn new() -> Self {
        Self {
            inner: Some(crate::Shell::builder()),
        }
    }

    /// Bind a host path into the VFS (copy mode).
    fn bind<'py>(
        slf: PyRefMut<'py, Self>,
        source: &str,
        destination: &str,
    ) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.bind(source, destination))
    }

    /// Bind a host path as read-only (copy mode).
    fn bind_readonly<'py>(
        slf: PyRefMut<'py, Self>,
        source: &str,
        destination: &str,
    ) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.bind_readonly(source, destination))
    }

    /// Bind a host path with direct passthrough.
    fn bind_direct<'py>(
        slf: PyRefMut<'py, Self>,
        source: &str,
        destination: &str,
    ) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.bind_direct(source, destination))
    }

    /// Bind a host path as read-only with direct passthrough.
    fn bind_direct_readonly<'py>(
        slf: PyRefMut<'py, Self>,
        source: &str,
        destination: &str,
    ) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.bind_direct_readonly(source, destination))
    }

    /// Add a bearer token credential for URLs matching a pattern.
    fn credential<'py>(
        slf: PyRefMut<'py, Self>,
        url_pattern: &str,
        token: &str,
    ) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| {
            b.credential(url_pattern, crate::CredKind::Bearer, token)
        })
    }

    /// Add a bearer token credential from an environment variable.
    fn credential_from_env<'py>(
        slf: PyRefMut<'py, Self>,
        url_pattern: &str,
        env_var: &str,
    ) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| {
            b.credential_from_env(url_pattern, crate::CredKind::Bearer, env_var)
        })
    }

    /// Set an environment variable.
    fn env<'py>(slf: PyRefMut<'py, Self>, key: &str, value: &str) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.env(key, value))
    }

    /// Set the umask for file creation (default: 0o022).
    fn umask(slf: PyRefMut<'_, Self>, umask: u32) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.umask(umask))
    }

    /// Set timeout in seconds.
    fn timeout(slf: PyRefMut<'_, Self>, seconds: f64) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.timeout(Duration::from_secs_f64(seconds)))
    }

    /// Set max recursion depth for functions/subshells.
    fn max_depth(slf: PyRefMut<'_, Self>, n: u32) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_depth(n))
    }

    /// Set max output size in bytes.
    fn max_output(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_output(n))
    }

    /// Set max file size in bytes.
    fn max_file_size(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_file_size(n))
    }

    /// Set max open file descriptors.
    fn max_fds(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_fds(n))
    }

    /// Set max concurrent background jobs.
    fn max_bg_jobs(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_bg_jobs(n))
    }

    /// Set max pipeline stages.
    fn max_pipeline(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_pipeline(n))
    }

    /// Set max input size for parser in bytes.
    fn max_input(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_input(n))
    }

    /// Set max inodes (files + directories) in VFS.
    fn max_inodes(slf: PyRefMut<'_, Self>, n: usize) -> PyResult<PyRefMut<'_, Self>> {
        chain(slf, |b| b.max_inodes(n))
    }

    /// Allow curl requests to URLs matching prefix (bypasses SSRF protection).
    fn allow_url<'py>(slf: PyRefMut<'py, Self>, prefix: &str) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.allow_url(prefix))
    }

    /// Load config from a TOML file.
    fn config_file<'py>(mut slf: PyRefMut<'py, Self>, path: &str) -> PyResult<PyRefMut<'py, Self>> {
        let b = slf
            .inner
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("builder consumed"))?;
        let updated = b
            .config_file(path)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        slf.inner = Some(updated);
        Ok(slf)
    }

    /// Load a Cedar authorization policy from a file.
    fn policy_file<'py>(mut slf: PyRefMut<'py, Self>, path: &str) -> PyResult<PyRefMut<'py, Self>> {
        let b = slf
            .inner
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("builder consumed"))?;
        let updated = b
            .policy_file(path)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        slf.inner = Some(updated);
        Ok(slf)
    }

    /// Set a Cedar authorization policy from a string.
    fn policy_str<'py>(slf: PyRefMut<'py, Self>, text: &str) -> PyResult<PyRefMut<'py, Self>> {
        chain(slf, |b| b.policy_str(text))
    }

    /// Build the Shell.
    fn build(&mut self) -> PyResult<Shell> {
        let builder = self
            .inner
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("builder consumed"))?;
        let shell = builder
            .build()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Shell::new(shell))
    }
}

/// A sandboxed shell environment.
#[pyclass(unsendable)]
pub struct Shell {
    inner: Option<crate::Shell>,
    runtime: tokio::runtime::Runtime,
}

impl Shell {
    fn new(shell: crate::Shell) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        Self {
            inner: Some(shell),
            runtime,
        }
    }
}

#[pymethods]
impl Shell {
    /// Create a new ShellBuilder.
    #[staticmethod]
    fn builder() -> ShellBuilder {
        ShellBuilder::new()
    }

    /// Run a command and capture output.
    fn run(&mut self, command: &str) -> PyResult<Output> {
        let shell = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        let local = tokio::task::LocalSet::new();
        let output = self.runtime.block_on(local.run_until(shell.run(command)));
        Ok(Output {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    /// Set an environment variable.
    fn set_env(&mut self, key: &str, value: &str) -> PyResult<()> {
        let shell = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        shell.set_env(key, value);
        Ok(())
    }

    /// Get an environment variable.
    fn get_env(&self, key: &str) -> PyResult<Option<String>> {
        let shell = self
            .inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        Ok(shell.get_env(key).map(|s| s.to_string()))
    }

    /// Read a file from the virtual filesystem as raw bytes.
    ///
    /// Mirrors `Sandbox.read_file` from the Strands SDK.
    fn read_file<'py>(&mut self, py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyBytes>> {
        let shell = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        let local = tokio::task::LocalSet::new();
        let bytes = self
            .runtime
            .block_on(local.run_until(shell.read_file(path)))
            .map_err(|e| native_file_error(py, path, &e))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Write raw bytes to a file in the virtual filesystem.
    ///
    /// Creates parent directories if missing (matches the `Sandbox.write_file`
    /// contract). Truncates any existing file at the path.
    fn write_file(&mut self, py: Python<'_>, path: &str, content: &[u8]) -> PyResult<()> {
        let shell = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        let local = tokio::task::LocalSet::new();
        self.runtime
            .block_on(local.run_until(shell.write_file(path, content)))
            .map_err(|e| native_file_error(py, path, &e))
    }

    /// Remove a file from the virtual filesystem.
    fn remove_file(&mut self, py: Python<'_>, path: &str) -> PyResult<()> {
        let shell = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        let local = tokio::task::LocalSet::new();
        self.runtime
            .block_on(local.run_until(shell.remove_file(path)))
            .map_err(|e| native_file_error(py, path, &e))
    }

    /// List entries in a directory, returning structured `FileInfo` objects.
    ///
    /// Names are basenames (no leading path), matching the `Sandbox.list_files`
    /// contract.
    fn list_files(&mut self, py: Python<'_>, path: &str) -> PyResult<Vec<FileInfo>> {
        let shell = self
            .inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        let local = tokio::task::LocalSet::new();
        let infos = self
            .runtime
            .block_on(local.run_until(shell.list_files(path)))
            .map_err(|e| native_file_error(py, path, &e))?;
        Ok(infos
            .into_iter()
            .map(|f| FileInfo {
                name: f.name,
                is_dir: f.is_dir,
                size: f.size,
            })
            .collect())
    }
}

/// Console-script entry point backing the `strands-shell` command.
///
/// Wired up via `[project.scripts]` so that `pip install strands-shell` /
/// `uvx strands-shell` place a `strands-shell` launcher on the user's PATH that
/// runs the full CLI — including `--mcp` (the stdio MCP server) — out of the
/// same wheel that ships the `_native` extension module. We reuse `sys.argv`
/// (rather than `std::env::args`) so the program name and arguments match what
/// the Python launcher received. Returns the process exit code; the caller (a
/// tiny generated `console_scripts` shim) passes it to `sys.exit`.
#[pyfunction]
fn cli_main(py: Python<'_>) -> PyResult<i32> {
    let argv: Vec<String> = py.import("sys")?.getattr("argv")?.extract()?;
    // Detach from the GIL while the CLI runs its own tokio runtime / blocking
    // REPL or MCP server loop, so the long-lived native loop doesn't hold the
    // GIL for its entire lifetime.
    let code = py.detach(|| crate::cli::run(argv));
    Ok(code)
}

/// Strands Shell native extension (`strands_shell._native`).
///
/// Low-level surface consumed by the pure-Python `strands_shell` package. Exposes the
/// builder, the `Shell` primitive, value types, and `NativeShellError`. The
/// customer-facing `Shell` / `Bind` / `Cred` / `Limits` / typed exceptions are
/// defined in `strands_shell/__init__.py` on top of these.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Shell>()?;
    m.add_class::<ShellBuilder>()?;
    m.add_class::<Output>()?;
    m.add_class::<FileInfo>()?;
    m.add("NativeShellError", m.py().get_type::<NativeShellError>())?;
    m.add_function(wrap_pyfunction!(cli_main, m)?)?;
    Ok(())
}
