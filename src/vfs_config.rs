use std::io;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

#[cfg(not(target_arch = "wasm32"))]
use crate::mcp_client::McpConfigEntry;
use crate::vfs::{self, LASH_GID, LASH_UID, ROOT_GID, ROOT_UID, Vfs};

/// The user-facing resource caps expressed in the TOML `[limits]` table.
///
/// This is a *config* type, deliberately separate from
/// [`crate::os::ProcessLimits`] (the runtime process-state type that is
/// re-armed on every MCP `tools/call`). It carries caps from two different
/// subsystems — process-level (`max_depth`, `max_output`, `max_fds`,
/// `max_bg_jobs`, `max_pipeline`, `max_input`, `timeout`) and VFS-level
/// (`max_file_size`, `max_inodes`) — and [`crate::shell::ShellBuilder::config_file`]
/// routes each field to where it belongs. Keeping them together here matches
/// how users think about "limits" without conflating the two runtime concepts.
///
/// Every field is optional: an omitted key leaves the builder's default in
/// place rather than resetting it. Unknown keys are rejected so typos like
/// `timeout_seconds` fail loudly instead of being silently ignored.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    // Process-level caps.
    pub max_depth: Option<u32>,
    pub max_output: Option<usize>,
    pub max_fds: Option<usize>,
    pub max_bg_jobs: Option<usize>,
    pub max_pipeline: Option<usize>,
    pub max_input: Option<usize>,
    /// Per-command wall-clock timeout, in whole seconds. Omit for no timeout;
    /// a value of `0` is rejected at build time (it would expire every command
    /// immediately — there is no "unlimited" sentinel).
    #[serde(deserialize_with = "deserialize_opt_timeout")]
    pub timeout: Option<Duration>,
    // VFS-level caps.
    pub max_file_size: Option<usize>,
    pub max_inodes: Option<usize>,
}

fn deserialize_opt_timeout<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Duration>, D::Error> {
    let secs: Option<u64> = Option::deserialize(d)?;
    Ok(secs.map(Duration::from_secs))
}

/// Configuration for initializing a VFS.
///
/// Example TOML:
/// ```toml
/// umask = "022"
///
/// [[bind]]
/// mode = "copy"
/// source = "/home/user/project"
/// destination = "/home/lash/project"
///
/// [[mcp]]
/// name = "my-server"
/// command = "/path/to/mcp-server"
/// args = ["--flag", "value"]
/// ```
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VfsConfig {
    #[serde(default = "default_umask", deserialize_with = "deserialize_octal")]
    pub umask: u32,
    #[serde(default)]
    pub bind: Vec<BindEntry>,
    #[serde(default)]
    pub cred: Vec<CredEntry>,
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub mcp: Vec<McpConfigEntry>,
    #[serde(default)]
    pub limits: Option<LimitsConfig>,
    /// SSRF allowlist — URL prefixes `curl` may reach. Mirrors the builder's
    /// `allow_url` / the bindings' `allowed_urls`.
    #[serde(default)]
    pub allowed_urls: Vec<String>,
    /// Environment variables seeded into the shell. A TOML `[env]` table.
    /// Ordered (BTreeMap) so config application is deterministic.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_umask() -> u32 {
    0o022
}

impl Default for VfsConfig {
    fn default() -> Self {
        Self {
            umask: default_umask(),
            bind: Vec::new(),
            cred: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            mcp: Vec::new(),
            limits: None,
            allowed_urls: Vec::new(),
            env: std::collections::BTreeMap::new(),
        }
    }
}

fn deserialize_octal<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let s = String::deserialize(d)?;
    u32::from_str_radix(&s, 8).map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindEntry {
    #[serde(default = "default_mode")]
    pub mode: BindMode,
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub readonly: bool,
}

fn default_mode() -> BindMode {
    BindMode::Copy
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindMode {
    Copy,
    Direct,
}

impl BindMode {
    /// The lowercase string the TOML / bindings use for this mode
    /// (`"copy"` or `"direct"`).
    pub fn as_str(self) -> &'static str {
        match self {
            BindMode::Copy => "copy",
            BindMode::Direct => "direct",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredEntry {
    pub url: String,
    #[serde(default)]
    pub methods: Vec<String>,
    pub kind: CredKind,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    /// Query parameter name (required for kind = "query")
    pub param: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredKind {
    Bearer,
    Query,
}

impl CredKind {
    /// The lowercase string the TOML / bindings use for this kind
    /// (`"bearer"` or `"query"`).
    pub fn as_str(self) -> &'static str {
        match self {
            CredKind::Bearer => "bearer",
            CredKind::Query => "query",
        }
    }
}

/// Parse a VFS config from a TOML string.
pub fn parse_config(toml_str: &str) -> io::Result<VfsConfig> {
    toml::from_str(toml_str).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))
}

/// A resolved credential ready for use at runtime.
#[derive(Clone)]
pub struct ResolvedCred {
    pub url: String,
    pub methods: Vec<String>,
    pub kind: CredKind,
    pub api_key: String,
    /// Query parameter name (for kind = Query)
    pub param: Option<String>,
}

/// Resolve credentials from config, reading env vars as needed.
pub fn resolve_creds(creds: &[CredEntry]) -> io::Result<Vec<ResolvedCred>> {
    creds
        .iter()
        .map(|c| {
            let api_key = if let Some(ref key) = c.api_key {
                key.clone()
            } else if let Some(ref env_var) = c.api_key_env {
                std::env::var(env_var).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("cred: environment variable {env_var} not set"),
                    )
                })?
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cred: must specify api_key or api_key_env",
                ));
            };
            // Validate that Query credentials have a param
            if matches!(c.kind, CredKind::Query) && c.param.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cred: kind=query requires param field",
                ));
            }
            Ok(ResolvedCred {
                url: c.url.clone(),
                methods: c.methods.iter().map(|m| m.to_uppercase()).collect(),
                kind: c.kind,
                api_key,
                param: c.param.clone(),
            })
        })
        .collect()
}

/// Build a VFS from a config.
pub fn build_vfs(config: &VfsConfig) -> io::Result<Vfs> {
    let mut vfs = Vfs::new();
    vfs.umask = config.umask;

    // Create standard directory structure
    vfs.mkdir("/home", 0o755, ROOT_UID, ROOT_GID)?;
    vfs.mkdir("/home/lash", 0o755, LASH_UID, LASH_GID)?;
    vfs.mkdir("/tmp", 0o1777, ROOT_UID, ROOT_GID)?;
    // Override /tmp mode since mkdir applies umask
    if let Ok(ino) = vfs.resolve("/tmp", true) {
        vfs.get_mut(ino)?.mode = 0o041777;
    }
    vfs.mkdir("/usr", 0o755, ROOT_UID, ROOT_GID)?;
    vfs.mkdir("/usr/bin", 0o755, ROOT_UID, ROOT_GID)?;
    vfs.mkdir("/bin", 0o755, ROOT_UID, ROOT_GID)?;
    vfs::create_dev_nodes(&mut vfs)?;
    vfs::create_bin_links(&mut vfs)?;

    // Process bind entries
    for bind in &config.bind {
        let src = Path::new(&bind.source);
        if !src.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("bind source not found: {}", bind.source),
            ));
        }
        // Ensure parent directories exist
        let dest_parent = {
            let d = vfs::normalize(&bind.destination);
            match d.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(i) => d[..i].to_string(),
            }
        };
        vfs.mkdir_p(&dest_parent, 0o755, LASH_UID, LASH_GID)?;

        match bind.mode {
            BindMode::Copy => {
                vfs::copy_from_host(&mut vfs, src, &bind.destination, LASH_UID, LASH_GID)?;
            }
            BindMode::Direct => {
                let meta = std::fs::symlink_metadata(src)?;
                let data = if meta.is_dir() {
                    vfs::InodeData::HostDir(bind.source.clone(), bind.readonly)
                } else {
                    vfs::InodeData::HostFile(bind.source.clone(), bind.readonly)
                };
                vfs.mknod(&bind.destination, data, 0o100644, LASH_UID, LASH_GID)?;
            }
        }
    }

    Ok(vfs)
}

/// Load a VFS config from a TOML file and build the VFS.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_config(path: &Path) -> io::Result<(Vfs, Vec<ResolvedCred>, Vec<McpConfigEntry>)> {
    let content = std::fs::read_to_string(path)?;
    let config = parse_config(&content)?;
    let creds = resolve_creds(&config.cred)?;
    let mcp = config.mcp.clone();
    let vfs = build_vfs(&config)?;
    Ok((vfs, creds, mcp))
}
