//! Integration tests for the read-only config snapshot returned by
//! [`Shell::config`].
//!
//! The snapshot lets an embedder introspect a constructed shell — binds, the
//! network allowlist, credential rules, env, umask, timeout, limits — without
//! having held onto the builder. Its most important guarantee is that it never
//! exposes resolved secret values: a credential reports only its source (a
//! literal was supplied, or the name of the env var it reads from).

use std::time::Duration;

use strands_shell::{CredKind, Shell};

#[test]
fn default_shell_reports_default_config() {
    let shell = Shell::builder().build().unwrap();
    let cfg = shell.config();
    assert!(cfg.binds.is_empty());
    assert!(cfg.credentials.is_empty());
    assert!(cfg.allowed_urls.is_empty());
    assert!(cfg.env.is_empty());
    assert_eq!(cfg.umask, 0o022);
    // Builder default is a 30s per-command timeout; the snapshot reports the
    // real effective value.
    assert_eq!(cfg.timeout_secs, Some(30.0));
    assert_eq!(cfg.limits.max_depth, 64);
    assert_eq!(cfg.limits.max_output, 1024 * 1024);
    assert_eq!(cfg.limits.max_fds, 128);
    assert_eq!(cfg.limits.max_bg_jobs, 8);
    assert_eq!(cfg.limits.max_pipeline, 16);
    assert_eq!(cfg.limits.max_input, 1024 * 1024);
    assert_eq!(cfg.limits.max_file_size, 10 * 1024 * 1024);
    assert_eq!(cfg.limits.max_inodes, 10_000);
}

#[test]
fn config_reports_binds() {
    let dir = tempdir();
    let shell = Shell::builder()
        .bind_direct_readonly(&dir, "/work")
        .bind(&dir, "/copy")
        .build()
        .unwrap();
    let cfg = shell.config();
    assert_eq!(cfg.binds.len(), 2);
    assert_eq!(cfg.binds[0].destination, "/work");
    assert_eq!(cfg.binds[0].mode, "direct");
    assert!(cfg.binds[0].readonly);
    assert_eq!(cfg.binds[1].destination, "/copy");
    assert_eq!(cfg.binds[1].mode, "copy");
    assert!(!cfg.binds[1].readonly);
}

#[test]
fn config_reports_allowed_urls_env_umask_timeout() {
    let shell = Shell::builder()
        .allow_url("https://api.example.com/")
        .allow_url("https://api.openai.com/")
        .env("PROJECT", "demo")
        .umask(0o027)
        .timeout(Duration::from_secs_f64(12.5))
        .build()
        .unwrap();
    let cfg = shell.config();
    assert_eq!(
        cfg.allowed_urls,
        vec![
            "https://api.example.com/".to_string(),
            "https://api.openai.com/".to_string()
        ]
    );
    assert_eq!(cfg.env, vec![("PROJECT".to_string(), "demo".to_string())]);
    assert_eq!(cfg.umask, 0o027);
    assert_eq!(cfg.timeout_secs, Some(12.5));
}

#[test]
fn config_reports_overridden_limits() {
    let shell = Shell::builder()
        .max_output(2048)
        .max_inodes(500)
        .build()
        .unwrap();
    let cfg = shell.config();
    assert_eq!(cfg.limits.max_output, 2048);
    assert_eq!(cfg.limits.max_inodes, 500);
}

#[test]
fn config_credentials_never_leak_literal_token() {
    let shell = Shell::builder()
        .credential(
            "https://api.example.com/*",
            CredKind::Bearer,
            "sk-super-secret",
        )
        .build()
        .unwrap();
    let cfg = shell.config();
    let cred = &cfg.credentials[0];
    assert_eq!(cred.url, "https://api.example.com/*");
    assert_eq!(cred.kind, "bearer");
    assert!(cred.from_literal);
    assert_eq!(cred.env_var, None);
    // The secret itself must never appear in the snapshot's debug form.
    let dump = format!("{cfg:?}");
    assert!(
        !dump.contains("sk-super-secret"),
        "literal token leaked into config snapshot: {dump}"
    );
}

#[test]
fn config_credentials_report_env_var_name_not_value() {
    // SAFETY: single-threaded test; we set and read back our own scoped var.
    unsafe {
        std::env::set_var("STRANDS_SHELL_RS_TEST_TOKEN", "value-must-not-leak");
    }
    let shell = Shell::builder()
        .credential_from_env(
            "https://api.openai.com/*",
            CredKind::Bearer,
            "STRANDS_SHELL_RS_TEST_TOKEN",
        )
        .build()
        .unwrap();
    let cfg = shell.config();
    let cred = &cfg.credentials[0];
    assert_eq!(cred.env_var.as_deref(), Some("STRANDS_SHELL_RS_TEST_TOKEN"));
    assert!(!cred.from_literal);
    let dump = format!("{cfg:?}");
    assert!(
        !dump.contains("value-must-not-leak"),
        "env-var secret leaked into config snapshot: {dump}"
    );
    unsafe {
        std::env::remove_var("STRANDS_SHELL_RS_TEST_TOKEN");
    }
}

#[test]
fn with_kernel_reports_default_config_snapshot() {
    // Shells built via with_kernel bypass the builder; they report the default
    // snapshot rather than panicking or carrying stale data.
    let kernel = Shell::builder().build().unwrap().kernel().clone();
    let shell = Shell::with_kernel(kernel);
    let cfg = shell.config();
    assert!(cfg.binds.is_empty());
    assert_eq!(cfg.umask, 0o022);
    assert_eq!(cfg.timeout_secs, None);
}

/// Create a throwaway host directory usable as a bind source (bind sources
/// must exist at build time).
fn tempdir() -> String {
    let mut path = std::env::temp_dir();
    let unique = format!(
        "strands-shell-config-rs-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    path.push(unique);
    std::fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().into_owned()
}
