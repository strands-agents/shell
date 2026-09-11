//! # Strands Shell — A Virtual Shell for AI Agents
//!
//! Strands Shell is a Bourne-compatible shell that runs entirely in userspace. It
//! provides a familiar Unix environment — `grep`, `cat`, `ls`, pipes,
//! redirections, variables — without ever calling `fork`/`exec` or making
//! direct system calls. Every operation flows through a pluggable [`os::Kernel`]
//! trait, giving you fine-grained control over what an AI agent can see and
//! do.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use strands_shell::Shell;
//!
//! # async fn example() -> std::io::Result<()> {
//! let mut shell = Shell::builder()
//!     .bind("/home/user/project", "/workspace")
//!     .build()?;
//!
//! let output = shell.run("ls /workspace").await;
//! println!("exit {}: {}", output.status, output.stdout);
//! # Ok(())
//! # }
//! ```
//!
//! ## Runtime Requirements
//!
//! Strands Shell uses [`tokio::task::spawn_local`] internally for pipeline stages,
//! so it must run inside a [`tokio::task::LocalSet`]:
//!
//! ```rust,no_run
//! use strands_shell::Shell;
//!
//! fn main() -> std::io::Result<()> {
//!     let mut shell = Shell::builder().build()?;
//!
//!     let rt = tokio::runtime::Builder::new_current_thread()
//!         .enable_all()
//!         .build()
//!         .unwrap();
//!     let local = tokio::task::LocalSet::new();
//!
//!     rt.block_on(local.run_until(async {
//!         let output = shell.run("echo hello | cat").await;
//!         assert_eq!(output.stdout.trim(), "hello");
//!     }));
//!     Ok(())
//! }
//! ```
//!
//! ## Sandboxing with Bind Mounts
//!
//! The shell starts with an empty virtual filesystem. Use bind mounts to
//! expose host paths:
//!
//! ```rust,no_run
//! # async fn example() -> std::io::Result<()> {
//! use strands_shell::Shell;
//!
//! let mut shell = Shell::builder()
//!     // Copy files into the VFS (isolated snapshot)
//!     .bind("/home/user/project", "/workspace")
//!     // Direct passthrough (reads/writes hit the real filesystem)
//!     .bind_direct("/tmp/scratch", "/scratch")
//!     // Read-only access
//!     .bind_direct_readonly("/etc/config", "/config")
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Network Credentials
//!
//! Inject credentials for HTTP requests made via `curl` inside the shell:
//!
//! ```rust,no_run
//! # async fn example() -> std::io::Result<()> {
//! use strands_shell::{CredKind, Shell};
//!
//! let mut shell = Shell::builder()
//!     .credential_from_env(
//!         "https://api.example.com/*",
//!         CredKind::Bearer,
//!         "API_TOKEN",
//!     )
//!     .build()?;
//!
//! let output = shell.run("curl https://api.example.com/data").await;
//! // The bearer token from $API_TOKEN is injected automatically
//! # Ok(())
//! # }
//! ```
//!
//! ## Resource Limits
//!
//! Constrain what the shell can do to prevent runaway agents:
//!
//! ```rust,no_run
//! # async fn example() -> std::io::Result<()> {
//! use std::time::Duration;
//! use strands_shell::Shell;
//!
//! let mut shell = Shell::builder()
//!     .timeout(Duration::from_secs(30))
//!     .max_depth(64)
//!     .max_output(1024 * 1024)     // 1 MB stdout cap
//!     .max_file_size(10 * 1024 * 1024) // 10 MB per file
//!     .max_fds(128)
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Custom Kernel Backends
//!
//! For full control, implement the [`os::Kernel`] trait and pass it to
//! [`Shell::with_kernel`]:
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use strands_shell::Shell;
//! use strands_shell::os::Kernel;
//!
//! fn from_custom_kernel(kernel: Arc<dyn Kernel>) -> Shell {
//!     Shell::with_kernel(kernel)
//! }
//! ```
//!
//! ## Architecture
//!
//! The crate is organized in layers:
//!
//! | Layer | Module | Purpose |
//! |-------|--------|---------|
//! | **Public API** | [`Shell`], [`ShellBuilder`], [`Output`] | Builder-based entry point |
//! | **Kernel** | [`os::Kernel`] | Trait abstracting all OS operations |
//! | **VFS Kernel** | [`vfs_kernel`] | Default kernel backed by an in-memory VFS |
//! | **VFS** | [`vfs`] | In-memory filesystem with host bind mounts |
//! | **Executor** | [`exec`] | Shell interpreter (parsing, expansion, pipelines) |
//! | **Commands** | [`commands`], [`builtins`] | Built-in command implementations |

pub mod builtins;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod commands;
pub mod exec;
pub mod io;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp_client;
pub mod os;
pub mod parser;
#[cfg(not(target_arch = "wasm32"))]
pub mod policy;
pub mod prelude;
pub mod shell;
pub mod vfs;
pub mod vfs_config;
pub mod vfs_kernel;

#[cfg(feature = "python")]
pub mod python;

#[cfg(feature = "node")]
pub mod js;

// Primary public API
pub use shell::{
    BindInfo, CredInfo, FileInfo, FileOpErrorKind, LimitsInfo, Output, Shell, ShellBuilder,
    ShellConfig,
};
pub use vfs_config::CredKind;
