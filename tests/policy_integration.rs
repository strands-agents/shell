//! Integration tests for Cedar authorization-policy support.
//!
//! These exercise the additive-restriction semantics end-to-end through the
//! public `Shell` API: no policy means unchanged behavior; a loaded policy can
//! only further restrict, never weaken the built-in SSRF / VFS checks.

use strands_shell::Shell;

fn rt() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    (rt, local)
}

/// No policy loaded → behavior is unchanged (default-allow).
#[test]
fn no_policy_is_unchanged() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell
            .run("echo hi > /home/lash/f.txt && cat /home/lash/f.txt")
            .await;
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.status, 0);
    }));
}

/// A policy that permits fs:read only for a specific path allows the matching
/// read and denies a non-matching one. A blanket permit for non-read actions
/// keeps the rest of the shell (writes, stats) functional.
#[test]
fn fs_read_permit_matches_path() {
    let policy = r#"
        permit(principal, action == Agent::Action::"fs:read", resource)
          when { context.input.path == "/home/lash/ok.txt" };
        permit(principal, action, resource)
          when { action != Agent::Action::"fs:read" };
    "#;
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().policy_str(policy).build().unwrap();
        // Setup writes are permitted by the blanket non-read rule.
        let setup = shell
            .run("echo ok > /home/lash/ok.txt; echo secret > /home/lash/secret.txt")
            .await;
        assert_eq!(setup.status, 0, "setup stderr: {}", setup.stderr);

        // Permitted read succeeds.
        let ok = shell.run("cat /home/lash/ok.txt").await;
        assert_eq!(ok.stdout, "ok\n");
        assert_eq!(ok.status, 0);

        // Non-matching read is denied.
        let denied = shell.run("cat /home/lash/secret.txt").await;
        assert_ne!(denied.status, 0);
        assert_eq!(denied.stdout, "");
    }));
}

/// Even when Cedar permits all net:request, the built-in SSRF protection still
/// blocks link-local / IMDS addresses — Cedar layers on top, never replaces it.
#[test]
fn ssrf_still_enforced_under_permit() {
    let policy = "permit(principal, action, resource);";
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().policy_str(policy).build().unwrap();
        let out = shell.run("curl http://169.254.169.254/").await;
        assert_ne!(out.status, 0, "IMDS fetch should be blocked");
        assert!(
            out.stderr.contains("curl:"),
            "expected curl error, got: {}",
            out.stderr
        );
    }));
}

/// A net:request that Cedar does NOT permit is denied even though the URL would
/// otherwise pass the SSRF check.
#[test]
fn net_request_denied_by_policy() {
    // Permits everything except network access.
    let policy =
        r#"permit(principal, action, resource) when { action != Agent::Action::"net:request" };"#;
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().policy_str(policy).build().unwrap();
        let out = shell.run("curl http://example.com/").await;
        assert_ne!(out.status, 0, "network should be denied by policy");
    }));
}

/// A malformed policy fails `build()`.
#[test]
fn malformed_policy_rejected_at_build() {
    let err = Shell::builder().policy_str("permit(garbage").build();
    assert!(err.is_err());
}

/// A syntactically valid policy referencing an action absent from the schema
/// fails schema validation at `build()`.
#[test]
fn unknown_action_rejected_at_build() {
    let policy = r#"permit(principal, action == Agent::Action::"fs:bogus", resource);"#;
    let err = Shell::builder().policy_str(policy).build();
    assert!(err.is_err());
}
