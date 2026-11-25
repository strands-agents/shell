use axum::{
    Router,
    extract::{self, Query},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{any, get},
};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;
use strands_shell::Shell;
use strands_shell::os::{
    DirEntry, Fd, FileStat, HttpRequest, HttpResponse, Kernel, OpenFlags, Process,
};

fn rt() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    (rt, local)
}

/// A host directory unique to this test, removed on drop.
struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{name}_{}_{n}", std::process::id()));
        std::fs::create_dir(&path).unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
        Self(path)
    }
}

impl std::ops::Deref for TestDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0)
            && !std::thread::panicking()
        {
            panic!("remove {}: {e}", self.0.display());
        }
    }
}

async fn start_server() -> String {
    let app = Router::new()
        .route("/hello", get(|| async { "Hello, World!" }))
        .route(
            "/json",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    r#"{"key":"value"}"#,
                )
            }),
        )
        .route("/echo", any(echo_handler))
        .route("/status/{code}", get(status_handler))
        .route("/redirect", get(|| async { Redirect::temporary("/hello") }))
        .route(
            "/redirect-slow",
            get(|| async { Redirect::temporary("/slow") }),
        )
        .route(
            "/redirect-rel",
            get(|| async { Redirect::temporary("hello") }),
        )
        .route("/large", get(|| async { "x".repeat(1000) }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                "late"
            }),
        )
        .route(
            "/slow-hop",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                Redirect::temporary("/slow-hop-end")
            }),
        )
        .route(
            "/slow-hop-end",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                "late"
            }),
        )
        .route(
            "/tricky-content-type",
            get(|| async {
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        r"text/plain;%{url_effective}\n",
                    )],
                    "body",
                )
            }),
        )
        .route(
            "/redirect-ftp",
            get(|| async { Redirect::temporary("ftp://example.com/file") }),
        )
        .route(
            "/method",
            any(|method: Method| async move { ([("x-method", method.to_string())], "body") }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::task::spawn_local(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn shell_with_server() -> (Shell, String) {
    let base = start_server().await;
    let shell = Shell::builder().allow_url(&base).build().unwrap();
    (shell, base)
}

/// A server that answers every path with the request target, `Host`, and
/// `Authorization` it received.
async fn start_target_echo_server() -> String {
    let app = Router::new().fallback(any(
        |target: axum::http::Uri, headers: HeaderMap| async move {
            let header = |name: &str| {
                headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("none")
                    .to_string()
            };
            format!(
                "target={target} host={} authorization={}",
                header("host"),
                header("authorization")
            )
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::task::spawn_local(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn start_redirect_server(location: String) -> String {
    let app = Router::new()
        .route(
            "/redirect",
            get(move || {
                let location = location.clone();
                async move { Redirect::temporary(&location) }
            }),
        )
        .route("/chain", get(|| async { Redirect::temporary("/redirect") }))
        .route("/echo", any(echo_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::task::spawn_local(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn echo_handler(
    method: Method,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> impl IntoResponse {
    let mut parts = vec![format!("method={}", method)];
    for name in [
        "content-type",
        "authorization",
        "cookie",
        "accept",
        "user-agent",
    ] {
        if let Some(v) = headers.get(name) {
            parts.push(format!("{}={}", name, v.to_str().unwrap_or("")));
        }
    }
    for (name, value) in &headers {
        if name.as_str().starts_with("x-") {
            parts.push(format!("{}={}", name, value.to_str().unwrap_or("")));
        }
    }
    let mut keys: Vec<_> = params.keys().collect();
    keys.sort();
    for key in keys {
        parts.push(format!("query_{}={}", key, params.get(key).unwrap()));
    }
    if !body.is_empty() {
        parts.push(format!("body={}", body));
    }
    parts.join("\n")
}

async fn status_handler(extract::Path(code): extract::Path<u16>) -> impl IntoResponse {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        format!("status {code}"),
    )
}

// ── Tests ───────────────────────────────────────────────────────────

#[test]
fn curl_basic_get() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl {base}/hello")).await;
        assert_eq!(out.stdout, "Hello, World!");
        assert_eq!(out.status, 0);
    }));
}

#[test]
fn curl_silent_flag() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -s {base}/hello")).await;
        assert_eq!(out.stdout, "Hello, World!");
    }));
}

#[test]
fn curl_post_data() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -d 'key=value' {base}/echo")).await;
        assert!(out.stdout.contains("method=POST"));
        assert!(out.stdout.contains("body=key=value"));
        assert!(
            out.stdout
                .contains("content-type=application/x-www-form-urlencoded")
        );
    }));
}

#[test]
fn curl_json_data() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(r#"curl --json '{{"a":1}}' {base}/echo"#))
            .await;
        assert!(out.stdout.contains("method=POST"));
        assert!(out.stdout.contains("content-type=application/json"));
        assert!(out.stdout.contains("accept=application/json"));
        assert!(out.stdout.contains(r#"body={"a":1}"#));
    }));
}

#[test]
fn curl_json_from_file() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        shell
            .run(r#"echo '{"from":"file"}' > /tmp/curl_json.txt"#)
            .await;
        let out = shell
            .run(&format!("curl --json @/tmp/curl_json.txt {base}/echo"))
            .await;
        assert!(out.stdout.contains(r#"body={"from":"file"}"#));
    }));
}

#[test]
fn curl_put_method() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -X PUT -d 'data' {base}/echo"))
            .await;
        assert!(out.stdout.contains("method=PUT"));
    }));
}

#[test]
fn curl_delete_method() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -X DELETE {base}/echo")).await;
        assert!(out.stdout.contains("method=DELETE"));
    }));
}

#[test]
fn curl_patch_method() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -X PATCH -d 'p' {base}/echo"))
            .await;
        assert!(out.stdout.contains("method=PATCH"));
    }));
}

#[test]
fn curl_head_method() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -X HEAD {base}/hello")).await;
        assert_eq!(out.stdout, "");
        assert_eq!(out.status, 0);
    }));
}

#[test]
fn curl_head_flag_prints_headers_only() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        for flag in ["-I", "--head", "-sI", "-Ii"] {
            let out = shell.run(&format!("curl {flag} {base}/method")).await;
            assert_eq!(out.status, 0, "{flag}");
            assert!(
                out.stdout.starts_with("HTTP/1.1 200 OK\r\n"),
                "{flag}: {}",
                out.stdout
            );
            assert!(
                out.stdout.contains("x-method: HEAD\r\n"),
                "{flag}: {}",
                out.stdout
            );
            assert!(out.stdout.ends_with("\r\n\r\n"), "{flag}: {}", out.stdout);
        }
    }));
}

#[test]
fn curl_head_flag_lets_request_pick_the_method() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -sI -X GET -w '%{{size_download}}' {base}/method"
            ))
            .await;
        assert_eq!(out.status, 0);
        assert!(out.stdout.contains("x-method: GET\r\n"), "{}", out.stdout);
        // The body is fetched but, as in curl, neither shown nor counted.
        assert!(out.stdout.ends_with("\r\n\r\n0"), "{}", out.stdout);
    }));
}

#[test]
fn curl_head_flag_follows_redirects_showing_every_hop() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -sIL {base}/redirect")).await;
        assert_eq!(out.status, 0);
        let blocks: Vec<&str> = out.stdout.split_terminator("\r\n\r\n").collect();
        assert_eq!(blocks.len(), 2, "{}", out.stdout);
        assert!(blocks[0].starts_with("HTTP/1.1 307 "), "{}", out.stdout);
        assert!(blocks[0].contains("location: /hello"), "{}", out.stdout);
        assert!(blocks[1].starts_with("HTTP/1.1 200 OK"), "{}", out.stdout);

        // Without `-L` only the redirect itself is shown.
        let out = shell.run(&format!("curl -sI {base}/redirect")).await;
        assert!(out.stdout.starts_with("HTTP/1.1 307 "), "{}", out.stdout);
        assert_eq!(out.stdout.matches("HTTP/").count(), 1, "{}", out.stdout);
    }));
}

#[test]
fn curl_head_flag_keeps_headers_when_a_later_hop_fails() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        for (path, flags, status, location) in [
            ("/redirect-ftp", "", 1, "ftp://example.com/file"),
            ("/redirect", "--proto-redir '=https'", 1, "/hello"),
            ("/redirect-slow", "-m 1 -w done", 28, "/slow"),
        ] {
            for output in ["", "-o /tmp/headers"] {
                let out = shell
                    .run(&format!("curl -sSIL {flags} {output} {base}{path}"))
                    .await;
                assert_eq!(out.status, status, "{path}: {}", out.stderr);
                assert!(!out.stderr.is_empty(), "{path}");
                let headers = if output.is_empty() {
                    out.stdout
                } else {
                    assert_eq!(out.stdout, if status == 28 { "done" } else { "" });
                    shell.run("cat /tmp/headers").await.stdout
                };
                assert!(headers.starts_with("HTTP/1.1 307 "), "{path}: {headers}");
                assert!(
                    headers.contains(&format!("\r\nlocation: {location}\r\n")),
                    "{path}: {headers}"
                );
                assert!(
                    headers.ends_with(if status == 28 && output.is_empty() {
                        "\r\n\r\ndone"
                    } else {
                        "\r\n\r\n"
                    }),
                    "{path}: {headers}"
                );
            }
        }
    }));
}

#[test]
fn curl_head_flag_keeps_headers_when_a_redirect_connection_fails() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", target_listener.local_addr().unwrap());
        let location = target.clone();
        let app = Router::new().route(
            "/away",
            get(move || async move { Redirect::temporary(&location) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::task::spawn_local(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Hold the target port, but close every connection without a response.
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = target_listener.accept().await {
                drop(stream);
            }
        });
        let mut shell = Shell::builder()
            .allow_url(&base)
            .allow_url(&target)
            .build()
            .unwrap();
        for output in ["", "-o /tmp/headers"] {
            let out = shell.run(&format!("curl -sSIL {output} {base}/away")).await;
            assert_eq!(out.status, 6, "{}", out.stderr);
            let headers = if output.is_empty() {
                out.stdout
            } else {
                assert_eq!(out.stdout, "");
                shell.run("cat /tmp/headers").await.stdout
            };
            assert!(headers.starts_with("HTTP/1.1 307 "), "{headers}");
            assert!(headers.contains(&format!("location: {target}\r\n")));
        }
    }));
}

#[test]
fn curl_head_flag_with_output_writes_headers_to_the_file() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -sI -o /tmp/head.txt -w '%{{http_code}}' {base}/hello"
            ))
            .await;
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "200");
        let out = shell.run("cat /tmp/head.txt").await;
        assert!(
            out.stdout.starts_with("HTTP/1.1 200 OK\r\n"),
            "{}",
            out.stdout
        );
        assert!(out.stdout.ends_with("\r\n\r\n"), "{}", out.stdout);
    }));
}

#[test]
fn curl_head_flag_with_fail_shows_headers_then_exits_22() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -sIf -w '%{{http_code}}' {base}/status/404"))
            .await;
        assert_eq!(out.status, 22);
        assert!(
            out.stdout.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{}",
            out.stdout
        );
        assert!(out.stdout.ends_with("\r\n\r\n404"), "{}", out.stdout);

        let out = shell
            .run(&format!(
                "curl -sIf -o /tmp/headers -w '%{{http_code}}' {base}/status/404"
            ))
            .await;
        assert_eq!(out.status, 22);
        assert_eq!(out.stdout, "404");
        let headers = shell.run("cat /tmp/headers").await.stdout;
        assert!(headers.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(headers.ends_with("\r\n\r\n"));
    }));
}

#[test]
fn curl_head_flag_conflicts_with_data() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -I -d x {base}/echo")).await;
        assert_eq!(out.status, 2);
        assert_eq!(out.stdout, "");
        assert!(
            out.stderr
                .contains("You can only select one HTTP request method!"),
            "{}",
            out.stderr
        );
        let out = shell.run(&format!("curl -s -I -d x {base}/echo")).await;
        assert_eq!(out.status, 2);
        assert_eq!(out.stderr, "");
    }));
}

#[test]
fn curl_custom_header() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -H 'X-Custom: test123' {base}/echo"))
            .await;
        assert!(out.stdout.contains("x-custom=test123"));
    }));
}

#[test]
fn curl_output_file() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -o /tmp/curl_out.txt {base}/hello"))
            .await;
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "");
        let content = shell.run("cat /tmp/curl_out.txt").await;
        assert_eq!(content.stdout, "Hello, World!");
    }));
}

#[test]
fn curl_fail_on_error() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -f {base}/status/404")).await;
        assert_eq!(out.status, 22);
    }));
}

#[test]
fn curl_fail_show_error() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -f -S {base}/status/500")).await;
        assert_eq!(out.status, 22);
        assert!(out.stderr.contains("22"));
    }));
}

#[test]
fn curl_no_fail_returns_zero() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl {base}/status/404")).await;
        assert_eq!(out.status, 0);
        assert!(out.stdout.contains("status 404"));
    }));
}

#[test]
fn curl_follow_redirect() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -L {base}/redirect")).await;
        assert_eq!(out.stdout, "Hello, World!");
    }));
}

#[test]
fn curl_protocol_relative_redirect_preserves_target_and_strips_credentials() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let target = start_server().await;
        let location = format!(
            "{}/echo?from=redirect",
            target.strip_prefix("http:").unwrap()
        );
        let source = start_redirect_server(location).await;
        let mut shell = Shell::builder()
            .allow_url(&source)
            .allow_url(&target)
            .credential(
                format!("{source}/"),
                strands_shell::CredKind::Bearer,
                "redirect-test-token",
            )
            .build()
            .unwrap();
        let initial = shell.run(&format!("curl {source}/echo")).await;
        assert!(
            initial
                .stdout
                .contains("authorization=Bearer redirect-test-token")
        );

        for path in ["redirect", "chain"] {
            let out = shell.run(&format!("curl -sSL {source}/{path}")).await;
            assert_eq!(out.status, 0, "{path}: {}", out.stderr);
            assert_eq!(
                out.stdout,
                format!(
                    "method=GET\naccept=*/*\nuser-agent=curl/{} (strands-shell)\nquery_from=redirect",
                    env!("CARGO_PKG_VERSION")
                ),
                "{path}"
            );
        }
    }));
}

#[test]
fn curl_protocol_relative_redirect_checks_target_allowlist() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let target = start_server().await;
        let location = format!("{}/hello", target.strip_prefix("http:").unwrap());
        let source = start_redirect_server(location).await;
        let mut shell = Shell::builder().allow_url(&source).build().unwrap();

        for path in ["redirect", "chain"] {
            let out = shell.run(&format!("curl -sSL {source}/{path}")).await;
            assert_eq!(out.status, 1, "{path}: {}", out.stderr);
            assert!(out.stderr.contains("denied"), "{path}: {}", out.stderr);
        }
    }));
}

#[test]
fn curl_protocol_relative_redirect_respects_proto_redir() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let target = start_server().await;
        let location = format!("{}/hello", target.strip_prefix("http:").unwrap());
        let source = start_redirect_server(location).await;
        let mut shell = Shell::builder()
            .allow_url(&source)
            .allow_url(&target)
            .build()
            .unwrap();

        // `-v` names the resolved hop, so this fails if `//host` is not resolved to the target.
        // Only `/redirect`: `--proto-redir` refuses `/chain` at its first, same-host hop.
        let out = shell
            .run(&format!(
                "curl -sSLv --proto-redir '=https' {source}/redirect"
            ))
            .await;
        assert_eq!(out.status, 1, "{}", out.stderr);
        let hop = format!("Redirecting to {target}/hello");
        assert!(out.stderr.contains(&hop), "{}", out.stderr);
        assert!(
            out.stderr.contains("Protocol \"http\" disabled"),
            "{}",
            out.stderr
        );
    }));
}

/// Forwards everything to the default kernel except HTTP, which it answers
/// from a script and records, so a test can use an `https://` origin
/// without a TLS server.
struct ScriptedHttpKernel {
    inner: std::sync::Arc<dyn Kernel>,
    routes: Vec<(&'static str, &'static str)>,
    requested: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Kernel for ScriptedHttpKernel {
    async fn http_request(&self, req: HttpRequest) -> io::Result<HttpResponse> {
        self.requested.lock().unwrap().push(req.url.clone());
        let location = self.routes.iter().find(|(url, _)| *url == req.url);
        let (status, headers, body) = match location {
            Some((_, loc)) => (302, vec![("location".into(), loc.to_string())], Vec::new()),
            None => (200, Vec::new(), b"ok".to_vec()),
        };
        Ok(HttpResponse {
            status,
            headers,
            body,
            version: "1.1".into(),
            reason: String::new(),
        })
    }

    fn new_process(&self) -> Process {
        self.inner.new_process()
    }

    async fn open(&self, proc: &mut Process, path: &str, flags: OpenFlags) -> io::Result<Fd> {
        self.inner.open(proc, path, flags).await
    }

    async fn list_dir(&self, proc: &Process, path: &str) -> io::Result<Vec<DirEntry>> {
        self.inner.list_dir(proc, path).await
    }

    async fn change_dir(&self, proc: &mut Process, path: &str) -> io::Result<()> {
        self.inner.change_dir(proc, path).await
    }

    async fn stat(&self, proc: &Process, path: &str) -> FileStat {
        self.inner.stat(proc, path).await
    }

    async fn lstat(&self, proc: &Process, path: &str) -> FileStat {
        self.inner.lstat(proc, path).await
    }

    async fn access(&self, proc: &Process, path: &str, mode: i32) -> bool {
        self.inner.access(proc, path, mode).await
    }

    async fn canonicalize(&self, proc: &Process, path: &str) -> io::Result<PathBuf> {
        self.inner.canonicalize(proc, path).await
    }

    async fn is_executable(&self, proc: &Process, path: &str) -> bool {
        self.inner.is_executable(proc, path).await
    }

    async fn glob(&self, proc: &Process, pattern: &str) -> Vec<String> {
        self.inner.glob(proc, pattern).await
    }

    fn isatty(&self, fd: i32) -> bool {
        self.inner.isatty(fd)
    }

    async fn remove_file(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.remove_file(proc, path).await
    }

    async fn remove_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.remove_dir(proc, path).await
    }

    async fn create_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.create_dir(proc, path).await
    }

    async fn rename(&self, proc: &Process, from: &str, to: &str) -> io::Result<()> {
        self.inner.rename(proc, from, to).await
    }

    async fn symlink(&self, proc: &Process, target: &str, link: &str) -> io::Result<()> {
        self.inner.symlink(proc, target, link).await
    }

    async fn read_link(&self, proc: &Process, path: &str) -> io::Result<String> {
        self.inner.read_link(proc, path).await
    }

    async fn set_permissions(&self, proc: &Process, path: &str, mode: u32) -> io::Result<()> {
        self.inner.set_permissions(proc, path, mode).await
    }

    fn now(&self) -> SystemTime {
        self.inner.now()
    }
}

#[test]
fn curl_protocol_relative_redirect_keeps_https_scheme() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let kernel = std::sync::Arc::new(ScriptedHttpKernel {
            inner: Shell::builder().build().unwrap().kernel().clone(),
            routes: vec![
                ("https://source.test/redirect", "//target.test/hello"),
                ("https://source.test/chain", "/redirect"),
            ],
            requested: std::sync::Mutex::new(Vec::new()),
        });
        let mut shell = Shell::with_kernel(kernel.clone());

        for path in ["redirect", "chain"] {
            kernel.requested.lock().unwrap().clear();
            let out = shell
                .run(&format!("curl -sSL https://source.test/{path}"))
                .await;
            assert_eq!(out.status, 0, "{path}: {}", out.stderr);
            assert_eq!(out.stdout, "ok", "{path}");
            let requested = kernel.requested.lock().unwrap();
            assert_eq!(
                requested.last().map(String::as_str),
                Some("https://target.test/hello"),
                "{path}: {requested:?}"
            );
        }
    }));
}

#[test]
fn curl_no_follow_redirect() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl {base}/redirect")).await;
        assert_eq!(out.status, 0);
    }));
}

#[test]
fn curl_include_headers() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -i {base}/hello")).await;
        assert!(out.stdout.contains("HTTP/"));
        assert!(out.stdout.contains("200"));
        assert!(out.stdout.contains("Hello, World!"));
    }));
}

#[test]
fn curl_verbose() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -v {base}/hello")).await;
        assert_eq!(out.stdout, "Hello, World!");
        assert!(out.stderr.contains("> GET"));
        assert!(out.stderr.contains("HTTP/"));
    }));
}

#[test]
fn curl_write_out_http_code() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                r#"curl -s -w '\ncode=%{{http_code}}' {base}/hello"#
            ))
            .await;
        assert!(out.stdout.contains("Hello, World!"));
        assert!(out.stdout.contains("code=200"));
    }));
}

#[test]
fn curl_write_out_size() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(r#"curl -s -w '%{{size_download}}' {base}/hello"#))
            .await;
        assert!(out.stdout.contains("13")); // "Hello, World!" = 13 bytes
    }));
}

#[test]
fn curl_cookies() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -b 'session=abc123' {base}/echo"))
            .await;
        assert!(out.stdout.contains("cookie=session=abc123"));
    }));
}

#[test]
fn curl_basic_auth() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -u user:pass {base}/echo")).await;
        assert!(out.stdout.contains("authorization=Basic"));
    }));
}

#[test]
fn curl_no_url() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("curl").await;
        assert_eq!(out.status, 2);
    }));
}

#[test]
fn curl_help() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("curl --help").await;
        assert_eq!(out.status, 0);
        assert!(out.stdout.contains("Usage: curl"));
    }));
}

#[test]
fn curl_credential_injection() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let mut shell = Shell::builder()
            .allow_url(&base)
            .credential(
                format!("{base}/"),
                strands_shell::CredKind::Bearer,
                "my-secret-token",
            )
            .build()
            .unwrap();
        let out = shell.run(&format!("curl {base}/echo")).await;
        assert!(out.stdout.contains("authorization=Bearer my-secret-token"));
    }));
}

#[test]
fn curl_allowed_url_via_toml_config() {
    // Positive proof that allowed_urls set via TOML relaxes SSRF identically to
    // the programmatic allow_url: the server is on loopback (127.0.0.1), which
    // is blocked by default, so a successful fetch means the TOML allowlist
    // entry took effect.
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let dir = TestDir::new("lsh_curl_toml_allow_test");
        let config_path = dir.join("allow.toml");
        std::fs::write(&config_path, format!("allowed_urls = [\"{base}/\"]\n")).unwrap();

        let mut shell = Shell::builder()
            .config_file(&config_path)
            .unwrap()
            .build()
            .unwrap();

        // In-list loopback URL is permitted and returns the body.
        let out = shell.run(&format!("curl {base}/hello")).await;
        assert_eq!(
            out.stdout, "Hello, World!",
            "TOML allowed_urls should permit the in-list loopback URL"
        );
    }));
}

#[test]
fn curl_relative_redirect() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -L {base}/redirect-rel")).await;
        assert_eq!(out.stdout, "Hello, World!");
    }));
}

#[test]
fn curl_max_output_limit() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let mut shell = Shell::builder()
            .allow_url(&base)
            .max_output(100)
            .build()
            .unwrap();
        let out = shell.run(&format!("curl {base}/large")).await;
        assert_ne!(out.status, 0);
    }));
}

#[test]
fn curl_blocked_localhost() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("curl http://localhost/test").await;
        assert_ne!(out.status, 0);
    }));
}

#[test]
fn curl_blocked_private_ip() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("curl http://192.168.1.1/test").await;
        assert_ne!(out.status, 0);
    }));
}

#[test]
fn curl_blocked_scheme() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        let out = shell.run("curl ftp://example.com/file").await;
        assert_ne!(out.status, 0);
    }));
}

// `localhost.` is refused before any connection, like `localhost`.
#[test]
fn curl_localhost_with_trailing_dot_denied() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let port = base.rsplit(':').next().unwrap();
        tokio::net::TcpStream::connect(format!("localhost.:{port}"))
            .await
            .expect("localhost. must resolve to the loopback server");
        let mut shell = Shell::builder().build().unwrap();
        let out = shell
            .run(&format!("curl http://localhost.:{port}/hello"))
            .await;
        assert_ne!(out.status, 0);
        assert!(out.stderr.contains("access denied"), "{}", out.stderr);
        assert_eq!(out.stdout, "");
    }));
}

#[test]
fn curl_redirect_to_blocked_ip_denied() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        // `base` is allowlisted and redirects to `target`, which deliberately is not.
        let target = start_server().await;
        let location = format!("{target}/hello");
        let app = Router::new().route(
            "/away",
            get(move || async move { Redirect::temporary(&location) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::task::spawn_local(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut shell = Shell::builder().allow_url(&base).build().unwrap();
        let out = shell.run(&format!("curl -v -L {base}/away")).await;
        assert_eq!(out.status, 1);
        assert_eq!(out.stdout, "");
        assert!(
            out.stderr
                .contains(&format!("Redirecting to {target}/hello")),
            "{}",
            out.stderr
        );
        assert!(
            out.stderr.contains("access denied: 127.0.0.1"),
            "{}",
            out.stderr
        );

        for output in ["", "-o /tmp/headers"] {
            let out = shell.run(&format!("curl -sSIL {output} {base}/away")).await;
            assert_eq!(out.status, 1);
            assert!(out.stderr.contains("access denied: 127.0.0.1"));
            let headers = if output.is_empty() {
                out.stdout
            } else {
                assert_eq!(out.stdout, "");
                shell.run("cat /tmp/headers").await.stdout
            };
            assert!(headers.starts_with("HTTP/1.1 307 "), "{headers}");
            assert!(headers.contains(&format!("location: {target}/hello\r\n")));
        }
    }));
}

// A2: userinfo injection must not smuggle a *blocked* host past the allowlist.
// Against an allowlist of `http://127.0.0.1:PORT`, the URL
// `http://127.0.0.1:PORT@169.254.169.254/` has a real host of 169.254.169.254
// (IMDS). The old string-prefix match accepted it (the `:` before `@` was a
// boundary char) and skipped the SSRF check entirely — a clean metadata escape.
// (Note: reaching a *public* host like evil.example.com is allowed by design —
// the allowlist is additive and only restricts internal hosts.)
#[test]
fn curl_allowlist_userinfo_injection_denied() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await; // http://127.0.0.1:PORT
        let mut shell = Shell::builder().allow_url(&base).build().unwrap();
        let host_port = base.trim_start_matches("http://");
        let evil = format!("http://{host_port}@169.254.169.254/");
        let out = shell.run(&format!("curl {evil}")).await;
        assert_ne!(
            out.status, 0,
            "userinfo injection to IMDS should be denied: {evil}"
        );
        assert!(
            out.stderr.contains("denied"),
            "expected SSRF denial for {evil}, got stderr: {}",
            out.stderr
        );
        // The genuine allowlisted URL still works.
        let ok = shell.run(&format!("curl {base}/hello")).await;
        assert_eq!(
            ok.status, 0,
            "allowlisted URL should still work: {}",
            ok.stderr
        );
    }));
}

#[test]
fn curl_query_credential_injection() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let dir = TestDir::new("lsh_query_cred_test");
        let config_path = dir.join("query_cred.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[[cred]]
url = "{base}/"
kind = "query"
api_key = "secret-token-123"
param = "api_key"
"#
            ),
        )
        .unwrap();
        let mut shell = Shell::builder()
            .allow_url(&base)
            .config_file(&config_path)
            .unwrap()
            .build()
            .unwrap();
        let out = shell.run(&format!("curl {base}/echo")).await;
        assert!(
            out.stdout.contains("query_api_key=secret-token-123"),
            "stdout: {}",
            out.stdout
        );
    }));
}

#[test]
fn curl_query_credential_appends_to_existing_query() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let dir = TestDir::new("lsh_query_cred_append_test");
        let config_path = dir.join("query_cred.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[[cred]]
url = "{base}/"
kind = "query"
api_key = "my-key"
param = "token"
"#
            ),
        )
        .unwrap();
        let mut shell = Shell::builder()
            .allow_url(&base)
            .config_file(&config_path)
            .unwrap()
            .build()
            .unwrap();
        let out = shell.run(&format!("curl '{base}/echo?foo=bar'")).await;
        assert!(
            out.stdout.contains("query_foo=bar"),
            "stdout: {}",
            out.stdout
        );
        assert!(
            out.stdout.contains("query_token=my-key"),
            "stdout: {}",
            out.stdout
        );
    }));
}

#[test]
fn curl_query_credential_requires_param() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let dir = TestDir::new("lsh_query_cred_no_param_test");
        let config_path = dir.join("query_cred.toml");
        std::fs::write(
            &config_path,
            r#"
[[cred]]
url = "https://example.com/"
kind = "query"
api_key = "secret"
"#,
        )
        .unwrap();
        let result = Shell::builder().config_file(&config_path).unwrap().build();
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.to_string().contains("query requires param field"),
            "error: {}",
            err
        );
    }));
}

/// The key of the `kind = "query"` credential in [`query_credential_shell`].
const QUERY_SECRET: &str = "query-secret-7f3a";

/// A shell that may reach `base` and adds [`QUERY_SECRET`] to every request for it.
fn query_credential_shell(base: &str, dir: &TestDir) -> Shell {
    let config_path = dir.join("query_cred.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[[cred]]
url = "{base}/"
kind = "query"
api_key = "{QUERY_SECRET}"
param = "api_key"
"#
        ),
    )
    .unwrap();
    Shell::builder()
        .allow_url(base)
        .config_file(&config_path)
        .unwrap()
        .build()
        .unwrap()
}

/// Assert that `out` is curl's report of a request that failed before a response,
/// and that the report leaves out the query credential.
fn assert_failed_without_secret(out: &strands_shell::Output) {
    assert_eq!(out.status, 6, "stderr: {}", out.stderr);
    assert!(
        out.stderr.starts_with("curl: (6) "),
        "stderr: {}",
        out.stderr
    );
    assert!(
        !out.stderr.contains(QUERY_SECRET) && !out.stdout.contains(QUERY_SECRET),
        "the query credential reached the agent: {}",
        out.stderr
    );
}

#[test]
fn curl_query_credential_stays_out_of_a_connection_error() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        // Hold the port, but close every connection without a response.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        let dir = TestDir::new("lsh_query_cred_connection_error");
        let mut shell = query_credential_shell(&base, &dir);
        let out = shell
            .run(&format!("curl -sS '{base}/geocode?q=zurich'"))
            .await;
        assert_failed_without_secret(&out);
    }));
}

#[test]
fn curl_query_credential_stays_out_of_a_dns_error() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        // `.invalid` never resolves (RFC 6761).
        let base = "http://strands-shell-test.invalid";
        let dir = TestDir::new("lsh_query_cred_dns_error");
        let mut shell = query_credential_shell(base, &dir);
        let out = shell.run(&format!("curl -sS {base}/geocode")).await;
        assert_failed_without_secret(&out);
    }));
}

#[test]
fn curl_query_credential_stays_out_of_a_url_too_long_to_send() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let dir = TestDir::new("lsh_query_cred_long_url");
        let mut shell = query_credential_shell(&base, &dir);
        // Longer than an HTTP request target can be, so the client gives up
        // before it connects.
        let path = "a".repeat(70_000);
        let out = shell.run(&format!("curl -sS {base}/{path}")).await;
        assert_failed_without_secret(&out);
    }));
}

#[test]
fn curl_path_credential_stays_inside_its_prefix() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_target_echo_server().await;
        let mut shell = Shell::builder()
            .allow_url(&base)
            .credential(
                format!("{base}/v1/projects/alpha/"),
                strands_shell::CredKind::Bearer,
                "alpha-token",
            )
            .build()
            .unwrap();
        let inside = shell
            .run(&format!("curl -s {base}/v1/projects/alpha/items"))
            .await;
        assert!(
            inside.stdout.ends_with("authorization=Bearer alpha-token"),
            "{}",
            inside.stdout
        );
        // Each path names a place outside `/v1/projects/alpha/`, once the client
        // or a server that decodes `%2F` resolves its dot segments.
        for (path, sent) in [
            (
                "/v1/projects/alpha/../beta/items",
                "/v1/projects/beta/items",
            ),
            (
                "/v1/projects/alpha/..\\beta/items",
                "/v1/projects/beta/items",
            ),
            (
                "/v1/projects/alpha/%2e%2e/%2e%2e/admin/keys",
                "/v1/admin/keys",
            ),
            (
                "/v1/projects/alpha/..%2Fbeta/items",
                "/v1/projects/alpha/..%2Fbeta/items",
            ),
            (
                "/v1/projects/alpha/..;/beta/items",
                "/v1/projects/alpha/..;/beta/items",
            ),
            (
                "/v1/projects/alpha/..;x=1/beta/items",
                "/v1/projects/alpha/..;x=1/beta/items",
            ),
        ] {
            let out = shell.run(&format!("curl -s '{base}{path}'")).await;
            assert_eq!(out.status, 0, "{path}: {}", out.stderr);
            assert!(
                out.stdout.starts_with(&format!("target={sent} ")),
                "{path}: {}",
                out.stdout
            );
            assert!(
                out.stdout.ends_with("authorization=none"),
                "{path} carried the credential: {}",
                out.stdout
            );
        }
    }));
}

#[test]
fn curl_refuses_a_host_header_that_names_another_host() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_target_echo_server().await;
        let authority = base.strip_prefix("http://").unwrap();
        let mut shell = Shell::builder()
            .allow_url(&base)
            .credential(
                format!("{base}/"),
                strands_shell::CredKind::Bearer,
                "host-token",
            )
            .build()
            .unwrap();
        for header in [
            "Host: other.example.test",
            "host: other.example.test",
            "Host: 127.0.0.1:1",
        ] {
            let out = shell
                .run(&format!("curl -s -H '{header}' {base}/items"))
                .await;
            assert_eq!(out.status, 1, "{header}: {}", out.stderr);
            assert_eq!(out.stdout, "", "{header}");
            assert!(
                out.stderr.contains("does not name the host of the URL"),
                "{header}: {}",
                out.stderr
            );
        }
        // A `Host` that names the URL's host goes out, with the credential.
        let out = shell
            .run(&format!("curl -s -H 'Host: {authority}' {base}/items"))
            .await;
        assert_eq!(
            out.stdout,
            format!("target=/items host={authority} authorization=Bearer host-token")
        );
    }));
}

#[test]
fn curl_refuses_a_forwarded_host_that_names_another_host() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_target_echo_server().await;
        let authority = base.strip_prefix("http://").unwrap();
        let mut shell = Shell::builder()
            .allow_url(&base)
            .credential(
                format!("{base}/"),
                strands_shell::CredKind::Bearer,
                "host-token",
            )
            .build()
            .unwrap();
        for header in [
            "X-Forwarded-Host: other.example.test".to_string(),
            format!("X-Forwarded-Host: {authority}, other.example.test"),
            "Forwarded: host=other.example.test".to_string(),
            "Forwarded: for=192.0.2.1;host=\"other.example.test\"".to_string(),
            "Forwarded: note=\"a;host=ignored.example\";host=other.example.test".to_string(),
            format!("Forwarded: host={authority}, host=\"other.example.test\""),
        ] {
            let out = shell
                .run(&format!("curl -s -H '{header}' {base}/items"))
                .await;
            assert_eq!(out.status, 1, "{header}: {}", out.stderr);
            assert_eq!(out.stdout, "", "{header}");
            assert!(
                out.stderr.contains("does not name the host of the URL"),
                "{header}: {}",
                out.stderr
            );
        }
        // Headers that name the URL's host, or carry no host, go out.
        for header in [
            format!("X-Forwarded-Host: {authority}"),
            format!("Forwarded: host={authority};proto=http"),
            "Forwarded: for=192.0.2.1".to_string(),
            "Forwarded: for=192.0.2.1;note=\"a;host=other.example.test\"".to_string(),
            "Forwarded: for=192.0.2.1;note=\"a,host=other.example.test\"".to_string(),
            format!("Forwarded: note=\"a;host=ignored.example\";host=\"{authority}\""),
            format!(
                "Forwarded: host=\"127\\.0.0.1:{}\"",
                authority.rsplit_once(':').unwrap().1
            ),
        ] {
            let out = shell
                .run(&format!("curl -s -H '{header}' {base}/items"))
                .await;
            assert_eq!(out.status, 0, "{header}: {}", out.stderr);
            assert!(
                out.stdout.ends_with("authorization=Bearer host-token"),
                "{header}: {}",
                out.stdout
            );
        }
    }));
}

#[test]
fn curl_empty_host_header_removes_it() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_target_echo_server().await;
        let authority = base.strip_prefix("http://").unwrap();
        let mut shell = Shell::builder().allow_url(&base).build().unwrap();
        // Shell removes the empty override but still derives Host from the URL.
        // System curl omits Host entirely for this option.
        let out = shell.run(&format!("curl -s -H 'Host:' {base}/items")).await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(
            out.stdout,
            format!("target=/items host={authority} authorization=none")
        );
    }));
}

#[test]
fn curl_content_type_removal_preserves_post_defaults_and_overrides() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        for (args, expected) in [
            ("-d a=b", Some("application/x-www-form-urlencoded")),
            ("-d a=b -H 'Content-Type: text/plain'", Some("text/plain")),
            ("-d a=b -H 'Content-Type:'", None),
            ("-H 'cOnTeNt-TyPe:   ' -d a=b", None),
            (
                "-H 'Content-Type:' -H 'Content-Type: text/plain' -d a=b",
                Some("text/plain"),
            ),
        ] {
            let out = shell.run(&format!("curl -s {args} {base}/echo")).await;
            assert_eq!(out.status, 0, "{args}: {}", out.stderr);
            assert!(out.stdout.contains("body=a=b"), "{args}: {}", out.stdout);
            let content_type = out
                .stdout
                .lines()
                .find_map(|line| line.strip_prefix("content-type="));
            assert_eq!(content_type, expected, "{args}: {}", out.stdout);
        }
    }));
}

#[test]
fn curl_refuses_malformed_forwarded_headers() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_target_echo_server().await;
        let mut shell = Shell::builder().allow_url(&base).build().unwrap();
        for value in [
            "host=\"unterminated",
            "note=\"dangling\\",
            "note=\"value\"host=other.example.test",
        ] {
            let out = shell
                .run(&format!("curl -s -H 'Forwarded: {value}' {base}/items"))
                .await;
            assert_eq!(out.status, 1, "{value}: {}", out.stderr);
            assert!(out.stdout.is_empty(), "{value}: {}", out.stdout);
            assert!(
                out.stderr.contains("access denied"),
                "{value}: {}",
                out.stderr
            );
        }
    }));
}

#[test]
fn curl_query_credential_goes_ahead_of_a_fragment() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let base = start_server().await;
        let dir = TestDir::new("lsh_query_cred_fragment_test");
        let config_path = dir.join("query_cred.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[[cred]]
url = "{base}/"
kind = "query"
api_key = "fragment-key"
param = "api_key"
"#
            ),
        )
        .unwrap();
        let mut shell = Shell::builder()
            .allow_url(&base)
            .config_file(&config_path)
            .unwrap()
            .build()
            .unwrap();
        for url in [format!("{base}/echo#part"), format!("{base}/echo?a=1#part")] {
            let out = shell.run(&format!("curl -s '{url}'")).await;
            assert_eq!(out.status, 0, "{url}: {}", out.stderr);
            assert!(
                out.stdout.contains("query_api_key=fragment-key"),
                "{url}: {}",
                out.stdout
            );
        }
    }));
}

// ── Options the Strands harness `web_fetch` tool sends ──────────────

#[test]
fn curl_globoff_is_accepted() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -g --globoff {base}/hello")).await;
        assert_eq!(out.stdout, "Hello, World!");
        assert_eq!(out.status, 0);
    }));
}

#[test]
fn curl_user_agent_sets_header() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -A 'agent/1.0' {base}/echo")).await;
        assert!(
            out.stdout.contains("user-agent=agent/1.0"),
            "{}",
            out.stdout
        );
        let out = shell
            .run(&format!("curl --user-agent 'agent/2.0' {base}/echo"))
            .await;
        assert!(
            out.stdout.contains("user-agent=agent/2.0"),
            "{}",
            out.stdout
        );
        let out = shell
            .run(&format!("curl -v -A 'agent/1.0' {base}/echo"))
            .await;
        let sent = out.stderr.matches("> User-Agent:").count();
        assert_eq!(sent, 1, "{}", out.stderr);
    }));
}

#[test]
fn curl_sends_a_default_user_agent() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl {base}/echo")).await;
        let expected = format!(
            "user-agent=curl/{} (strands-shell)",
            env!("CARGO_PKG_VERSION")
        );
        assert!(out.stdout.contains(&expected), "{}", out.stdout);
    }));
}

#[test]
fn curl_header_user_agent_replaces_the_default() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -v -H 'User-Agent: explicit' {base}/echo"))
            .await;
        assert!(out.stdout.contains("user-agent=explicit"), "{}", out.stdout);
        assert!(!out.stdout.contains("strands-shell"), "{}", out.stdout);
        let sent = out.stderr.matches("> User-Agent:").count();
        assert_eq!(sent, 1, "{}", out.stderr);
    }));
}

// A header name matches in any letter case.
#[test]
fn curl_lowercase_user_agent_header_replaces_the_default() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -v -H 'user-agent: lower' {base}/echo"))
            .await;
        assert!(out.stdout.contains("user-agent=lower"), "{}", out.stdout);
        assert!(!out.stderr.contains("strands-shell"), "{}", out.stderr);
    }));
}

#[test]
fn curl_header_user_agent_overrides_a() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -A 'agent/1.0' -H 'User-Agent: explicit' {base}/echo"
            ))
            .await;
        assert!(out.stdout.contains("user-agent=explicit"), "{}", out.stdout);
        assert!(!out.stdout.contains("agent/1.0"), "{}", out.stdout);
    }));
}

#[test]
fn curl_max_time_exits_28() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let started = std::time::Instant::now();
        let out = shell
            .run(&format!("curl -sS --max-time 0.2 {base}/slow"))
            .await;
        assert_eq!(out.status, 28);
        assert!(out.stderr.contains("(28)"), "{}", out.stderr);
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
    }));
}

#[test]
fn curl_max_time_allows_a_fast_transfer() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -m 10 {base}/hello")).await;
        assert_eq!(out.stdout, "Hello, World!");
        assert_eq!(out.status, 0);
    }));
}

#[test]
fn curl_max_time_rejects_a_non_number() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl --max-time soon {base}/hello"))
            .await;
        assert_eq!(out.status, 2);
    }));
}

#[test]
fn curl_proto_permits_listed_protocols() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl --proto '=http,https' {base}/hello"))
            .await;
        assert_eq!(out.stdout, "Hello, World!");
        assert_eq!(out.status, 0);
    }));
}

#[test]
fn curl_proto_refuses_unlisted_protocol() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        for list in ["=https", "-http", "-all,+https"] {
            let out = shell
                .run(&format!("curl -sS --proto '{list}' {base}/hello"))
                .await;
            assert_eq!(out.status, 1, "--proto {list}");
            assert_eq!(out.stdout, "", "--proto {list}");
            assert!(
                out.stderr.contains("Protocol \"http\" disabled"),
                "{}",
                out.stderr
            );
        }
    }));
}

#[test]
fn curl_proto_rejects_a_malformed_list() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl --proto '*http' {base}/hello"))
            .await;
        assert_eq!(out.status, 2);
    }));
}

#[test]
fn curl_proto_redir_refuses_a_redirect() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -sS -L --proto-redir '=https' {base}/redirect"
            ))
            .await;
        assert_eq!(out.status, 1);
        assert_eq!(out.stdout, "");
        // The first request is not a redirect, so `--proto-redir` does not judge it.
        let out = shell
            .run(&format!("curl --proto-redir '=https' {base}/hello"))
            .await;
        assert_eq!(out.stdout, "Hello, World!");
    }));
}

#[test]
fn curl_proto_redir_refuses_a_non_http_redirect() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -sS -L --proto '=http,https' --proto-redir '=http,https' {base}/redirect-ftp"
            ))
            .await;
        assert_eq!(out.status, 1);
        assert!(
            out.stderr.contains("Protocol \"ftp\" not supported"),
            "{}",
            out.stderr
        );
    }));
}

#[test]
fn curl_write_out_content_type() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -s -w '\\n%{{content_type}}' {base}/json"))
            .await;
        assert_eq!(out.stdout, "{\"key\":\"value\"}\napplication/json");
    }));
}

#[test]
fn curl_write_out_url_effective() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -sL -w '\\n%{{url_effective}}' {base}/redirect"
            ))
            .await;
        assert_eq!(out.stdout, format!("Hello, World!\n{base}/hello"));
        let out = shell
            .run(&format!("curl -s -w '%{{url_effective}}' {base}/hello"))
            .await;
        assert_eq!(out.stdout, format!("Hello, World!{base}/hello"));
    }));
}

// The command line `web_fetch` sends: the body goes to the file, `-w` to stdout.
#[test]
fn curl_web_fetch_command_line() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -sSL -g --fail --proto '=http,https' --proto-redir '=http,https' \
                 --max-time 30 -A 'web-fetch/1.0' -o /tmp/page.txt \
                 -w '%{{content_type}}\\n%{{url_effective}}' -- {base}/redirect; \
                 echo; cat /tmp/page.txt"
            ))
            .await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(
            out.stdout,
            format!("text/plain; charset=utf-8\n{base}/hello\nHello, World!")
        );
    }));
}

#[test]
fn curl_write_out_reaches_stdout_with_output() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -s -o /tmp/body.txt -w '%{{http_code}}' {base}/hello; echo; cat /tmp/body.txt"
            ))
            .await;
        assert_eq!(out.stdout, "200\nHello, World!");
    }));
}

#[test]
fn curl_include_with_output_writes_headers_to_the_file() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -s -i -o /tmp/inc.txt {base}/hello"))
            .await;
        assert_eq!(out.stdout, "");
        let out = shell.run("cat /tmp/inc.txt").await;
        assert!(
            out.stdout.starts_with("HTTP/1.1 200 OK\r\n"),
            "{}",
            out.stdout
        );
        assert!(
            out.stdout.ends_with("\r\n\r\nHello, World!"),
            "{}",
            out.stdout
        );
    }));
}

#[test]
fn curl_write_out_on_fail() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl -sf -w '%{{http_code}}' {base}/status/404"))
            .await;
        assert_eq!(out.status, 22);
        assert_eq!(out.stdout, "404");
    }));
}

#[test]
fn curl_write_out_on_timeout() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -s -m 0.2 -w '%{{http_code}} %{{url_effective}}' {base}/slow"
            ))
            .await;
        assert_eq!(out.status, 28);
        assert_eq!(out.stdout, format!("000 {base}/slow"));
    }));
}

// A server's `Content-Type` is written as sent, never expanded again.
#[test]
fn curl_write_out_does_not_expand_a_substituted_value() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!(
                "curl -s -o /dev/null -w '%{{content_type}}|' {base}/tricky-content-type"
            ))
            .await;
        assert_eq!(out.stdout, r"text/plain;%{url_effective}\n|");
    }));
}

#[test]
fn curl_max_time_spans_redirect_hops() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let started = std::time::Instant::now();
        let out = shell
            .run(&format!(
                "curl -sL -m 0.8 -w '%{{url_effective}}' {base}/slow-hop"
            ))
            .await;
        assert_eq!(out.status, 28);
        assert_eq!(out.stdout, format!("{base}/slow-hop-end"));
        let elapsed = started.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(800),
            "the deadline spans both hops, so it must be >= 800ms, got {elapsed:?}"
        );
    }));
}

#[test]
fn curl_max_time_zero_and_huge_are_no_limit() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        for limit in ["0", "1e19", "1e300", "99999999999999999999"] {
            let out = shell.run(&format!("curl -m {limit} {base}/hello")).await;
            assert_eq!(out.status, 0, "-m {limit}: {}", out.stderr);
            assert_eq!(out.stdout, "Hello, World!", "-m {limit}");
        }
    }));
}

#[test]
fn curl_empty_user_agent_sends_none() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell.run(&format!("curl -A '' {base}/echo")).await;
        assert!(!out.stdout.contains("user-agent="), "{}", out.stdout);
        assert!(!out.stdout.contains("strands-shell"), "{}", out.stdout);
    }));
}

// Each `--proto` replaces the one before it.
#[test]
fn curl_last_proto_wins() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let (mut shell, base) = shell_with_server().await;
        let out = shell
            .run(&format!("curl --proto -http --proto -https {base}/hello"))
            .await;
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "Hello, World!");
    }));
}
