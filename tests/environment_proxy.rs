//! `curl` ignores proxy variables in the host process environment.
//!
//! This file holds one test, so it runs in its own process and its environment
//! changes reach no other test.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use strands_shell::Shell;

#[test]
fn curl_does_not_send_an_injected_credential_to_an_environment_proxy() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(async {
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = connections.clone();
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = proxy.accept().await {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });

        // SAFETY: this test binary runs only this test, and no other thread
        // reads the environment while these variables change.
        unsafe {
            for name in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ] {
                std::env::set_var(name, &proxy_url);
            }
            std::env::remove_var("NO_PROXY");
            std::env::remove_var("no_proxy");
        }

        // `.invalid` never resolves (RFC 6761), so the request can succeed
        // only if a proxy carries it.
        let target = "http://credential-target.invalid";
        let mut shell = Shell::builder()
            .allow_url(target)
            .credential(
                format!("{target}/"),
                strands_shell::CredKind::Bearer,
                "environment-proxy-secret",
            )
            .build()
            .unwrap();
        let out = shell.run(&format!("curl -sS {target}/x")).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let shown = format!(
            "status={} stdout={:?} stderr={:?}",
            out.status, out.stdout, out.stderr
        );
        assert_eq!(
            connections.load(Ordering::SeqCst),
            0,
            "curl connected to the environment proxy: {shown}"
        );
        assert_ne!(out.status, 0, "the unresolvable target must fail: {shown}");
        assert!(
            !out.stdout.contains("environment-proxy-secret")
                && !out.stderr.contains("environment-proxy-secret"),
            "the credential must not appear in output: {shown}"
        );
    }));
}
