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

// --------------------------------------------------------------------------- #
// Read-only config snapshot carriers
//
// These mirror the Rust `crate::shell::{ShellConfig, BindInfo, CredInfo,
// LimitsInfo}` view types. They are the low-level surface; the pure-Python
// wrapper (`strands_shell/__init__.py`) re-shapes them into frozen public
// dataclasses. Secret values are never carried — see `CredInfo`.
// --------------------------------------------------------------------------- #

/// A single bind mount in a config snapshot.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct BindInfo {
    #[pyo3(get)]
    pub source: String,
    #[pyo3(get)]
    pub destination: String,
    /// `"copy"` or `"direct"`.
    #[pyo3(get)]
    pub mode: String,
    #[pyo3(get)]
    pub readonly: bool,
}

#[pymethods]
impl BindInfo {
    fn __repr__(&self) -> String {
        format!(
            "BindInfo(source={:?}, destination={:?}, mode={:?}, readonly={})",
            self.source,
            self.destination,
            self.mode,
            if self.readonly { "True" } else { "False" }
        )
    }
}

/// A single credential rule in a config snapshot. Never carries the secret.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct CredInfo {
    #[pyo3(get)]
    pub url: String,
    /// `"bearer"` or `"query"`.
    #[pyo3(get)]
    pub kind: String,
    #[pyo3(get)]
    pub methods: Vec<String>,
    #[pyo3(get)]
    pub param: Option<String>,
    /// Name of the env var the secret is read from, or `None` for a literal.
    #[pyo3(get)]
    pub env_var: Option<String>,
    /// True when a literal token was supplied (value itself never exposed).
    #[pyo3(get)]
    pub from_literal: bool,
}

#[pymethods]
impl CredInfo {
    fn __repr__(&self) -> String {
        format!(
            "CredInfo(url={:?}, kind={:?}, methods={:?}, param={:?}, env_var={:?}, from_literal={})",
            self.url,
            self.kind,
            self.methods,
            self.param,
            self.env_var,
            if self.from_literal { "True" } else { "False" }
        )
    }
}

/// Resource caps in a config snapshot.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct LimitsInfo {
    #[pyo3(get)]
    pub max_depth: u32,
    #[pyo3(get)]
    pub max_output: usize,
    #[pyo3(get)]
    pub max_fds: usize,
    #[pyo3(get)]
    pub max_bg_jobs: usize,
    #[pyo3(get)]
    pub max_pipeline: usize,
    #[pyo3(get)]
    pub max_input: usize,
    #[pyo3(get)]
    pub max_file_size: usize,
    #[pyo3(get)]
    pub max_inodes: usize,
}

#[pymethods]
impl LimitsInfo {
    fn __repr__(&self) -> String {
        format!(
            "LimitsInfo(max_depth={}, max_output={}, max_fds={}, max_bg_jobs={}, max_pipeline={}, max_input={}, max_file_size={}, max_inodes={})",
            self.max_depth,
            self.max_output,
            self.max_fds,
            self.max_bg_jobs,
            self.max_pipeline,
            self.max_input,
            self.max_file_size,
            self.max_inodes
        )
    }
}

/// A read-only snapshot of how a `Shell` was configured.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct ShellConfig {
    #[pyo3(get)]
    pub binds: Vec<BindInfo>,
    #[pyo3(get)]
    pub credentials: Vec<CredInfo>,
    #[pyo3(get)]
    pub allowed_urls: Vec<String>,
    /// List of `(key, value)` pairs, in declaration order.
    #[pyo3(get)]
    pub env: Vec<(String, String)>,
    #[pyo3(get)]
    pub umask: u32,
    /// Per-command timeout in seconds, or `None` for no timeout.
    #[pyo3(get)]
    pub timeout: Option<f64>,
    #[pyo3(get)]
    pub limits: LimitsInfo,
}

#[pymethods]
impl ShellConfig {
    fn __repr__(&self) -> String {
        format!(
            "ShellConfig(binds={} entries, credentials={} entries, allowed_urls={:?}, env={} vars, umask={:#o}, timeout={:?})",
            self.binds.len(),
            self.credentials.len(),
            self.allowed_urls,
            self.env.len(),
            self.umask,
            self.timeout
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

    /// Read-only snapshot of the configuration this shell was built with.
    ///
    /// Mirrors `Shell::config()` in the core. Never carries secret values —
    /// each credential reports its source (literal vs env-var name) only.
    fn config(&self) -> PyResult<ShellConfig> {
        let shell = self
            .inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("shell consumed"))?;
        let c = shell.config();
        Ok(ShellConfig {
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
            env: c.env.clone(),
            umask: c.umask,
            timeout: c.timeout_secs,
            limits: LimitsInfo {
                max_depth: c.limits.max_depth,
                max_output: c.limits.max_output,
                max_fds: c.limits.max_fds,
                max_bg_jobs: c.limits.max_bg_jobs,
                max_pipeline: c.limits.max_pipeline,
                max_input: c.limits.max_input,
                max_file_size: c.limits.max_file_size,
                max_inodes: c.limits.max_inodes,
            },
        })
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
    m.add_class::<ShellConfig>()?;
    m.add_class::<BindInfo>()?;
    m.add_class::<CredInfo>()?;
    m.add_class::<LimitsInfo>()?;
    m.add("NativeShellError", m.py().get_type::<NativeShellError>())?;
    m.add_function(wrap_pyfunction!(cli_main, m)?)?;
    Ok(())
}
