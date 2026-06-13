use std::io::Cursor;

use serde_json::{Value, json};
use strands_shell::Shell;

fn rt() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    (rt, local)
}

/// Run an in-process MCP session using serve_io, returning parsed response lines.
fn mcp_session(requests: &[Value]) -> Vec<Value> {
    let mut input = String::new();
    for req in requests {
        input.push_str(&serde_json::to_string(req).unwrap());
        input.push('\n');
    }

    let (rt, local) = rt();
    let output = rt.block_on(local.run_until(async {
        let shell = Shell::builder().build().unwrap();
        let kernel = shell.kernel().clone();
        let limits = shell.limits();
        let mut cursor = Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        strands_shell::mcp::serve_io(kernel, &limits, &mut cursor, &mut out).await;
        out
    }));

    let stdout = String::from_utf8(output).unwrap();
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("invalid JSON response"))
        .collect()
}

fn init_msg(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "test", "version": "0.1"}
    }})
}

fn initialized_msg() -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
}

fn tool_call(id: u64, tool: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
        "name": tool, "arguments": args
    }})
}

/// Helper: init + call a tool, return the tool response.
fn mcp_tool(tool: &str, args: Value) -> Value {
    let responses = mcp_session(&[init_msg(1), initialized_msg(), tool_call(2, tool, args)]);
    assert!(
        responses.len() >= 2,
        "expected >=2 responses, got {}",
        responses.len()
    );
    responses[1].clone()
}

/// Helper: init + send a method, return the response.
fn mcp_method(id: u64, method: &str, params: Value) -> Value {
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    ]);
    assert!(responses.len() >= 2);
    responses[1].clone()
}

// ── Initialize ──────────────────────────────────────────────────────

#[test]
fn mcp_initialize() {
    let responses = mcp_session(&[init_msg(1)]);
    assert_eq!(responses.len(), 1);
    let r = &responses[0];
    assert_eq!(r["jsonrpc"], "2.0");
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert_eq!(r["result"]["serverInfo"]["name"], "strands-shell");
}

// ── tools/list ──────────────────────────────────────────────────────

#[test]
fn mcp_tools_list() {
    let r = mcp_method(2, "tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"shell"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"list_dir"));
    assert_eq!(names.len(), 4);
}

// ── shell tool ──────────────────────────────────────────────────────

#[test]
fn mcp_shell_echo() {
    let r = mcp_tool("shell", json!({"command": "echo hello"}));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text.trim(), "hello");
}

#[test]
fn mcp_shell_exit_code() {
    let r = mcp_tool("shell", json!({"command": "false"}));
    assert_eq!(r["result"]["metadata"]["exit_code"], 1);
}

#[test]
fn mcp_shell_pipeline() {
    let r = mcp_tool("shell", json!({"command": "echo hello | tr a-z A-Z"}));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text.trim(), "HELLO");
}

#[test]
fn mcp_shell_stderr() {
    let r = mcp_tool("shell", json!({"command": "echo err >&2"}));
    // content[1] is stderr; content[0] (stdout) is empty here.
    let stderr = r["result"]["content"][1]["text"].as_str().unwrap();
    assert!(stderr.contains("err"), "stderr: {stderr}");
    assert_eq!(r["result"]["content"][0]["text"].as_str().unwrap(), "");
}

#[test]
fn mcp_shell_missing_command() {
    let r = mcp_tool("shell", json!({}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("command"), "text: {text}");
}

#[test]
fn mcp_shell_timeout() {
    let r = mcp_tool("shell", json!({"command": "echo fast", "timeout_ms": 5000}));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text.trim(), "fast");
}

#[test]
fn mcp_shell_stdout_and_stderr() {
    let r = mcp_tool("shell", json!({"command": "echo out && echo err >&2"}));
    // Streams are split: stdout in content[0], stderr in content[1].
    let stdout = r["result"]["content"][0]["text"].as_str().unwrap();
    let stderr = r["result"]["content"][1]["text"].as_str().unwrap();
    assert!(stdout.contains("out"), "stdout: {stdout}");
    assert!(!stdout.contains("err"), "stdout leaked stderr: {stdout}");
    assert!(stderr.contains("err"), "stderr: {stderr}");
}

#[test]
fn mcp_shell_state_persists_across_calls() {
    // cwd, exported env vars, and shell functions all set in one tools/call
    // must be visible in the next tools/call on the same connection.
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        tool_call(
            2,
            "shell",
            json!({"command": "mkdir -p /tmp/work && cd /tmp/work && export GREETING=hello && greet() { echo \"$GREETING $1\"; }"}),
        ),
        tool_call(3, "shell", json!({"command": "pwd"})),
        tool_call(4, "shell", json!({"command": "echo $GREETING"})),
        tool_call(5, "shell", json!({"command": "greet world"})),
    ]);
    assert_eq!(responses.len(), 5);

    let pwd = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(pwd.contains("/tmp/work"), "pwd output: {pwd}");

    let env = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(env.contains("hello"), "env output: {env}");

    let func = responses[4]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(func.contains("hello world"), "function output: {func}");
}

// ── read_file tool ──────────────────────────────────────────────────

#[test]
fn mcp_read_file() {
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        tool_call(
            2,
            "shell",
            json!({"command": "printf 'line1\\nline2\\nline3\\n' > /tmp/rf.txt"}),
        ),
        tool_call(3, "read_file", json!({"file_path": "/tmp/rf.txt"})),
    ]);
    let text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("line1"), "text: {text}");
    assert!(text.contains("line2"), "text: {text}");
    assert!(text.contains("line3"), "text: {text}");
}

#[test]
fn mcp_read_file_offset_limit() {
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        tool_call(
            2,
            "shell",
            json!({"command": "printf 'a\\nb\\nc\\nd\\ne\\n' > /tmp/rfo.txt"}),
        ),
        tool_call(
            3,
            "read_file",
            json!({"file_path": "/tmp/rfo.txt", "offset": 2, "limit": 2}),
        ),
    ]);
    let text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("b"), "should contain line 2: {text}");
    assert!(text.contains("c"), "should contain line 3: {text}");
    assert!(
        text.contains("more lines"),
        "should show truncation: {text}"
    );
}

#[test]
fn mcp_read_file_missing_path() {
    let r = mcp_tool("read_file", json!({}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
}

#[test]
fn mcp_read_file_nonexistent() {
    let r = mcp_tool("read_file", json!({"file_path": "/nonexistent.txt"}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
}

// ── write_file tool ─────────────────────────────────────────────────

#[test]
fn mcp_write_file() {
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        tool_call(
            2,
            "write_file",
            json!({"file_path": "/tmp/wf.txt", "content": "hello world"}),
        ),
        tool_call(3, "read_file", json!({"file_path": "/tmp/wf.txt"})),
    ]);
    let write_text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        write_text.contains("11 bytes"),
        "write result: {write_text}"
    );
    let read_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        read_text.contains("hello world"),
        "read result: {read_text}"
    );
}

#[test]
fn mcp_write_file_missing_path() {
    let r = mcp_tool("write_file", json!({"content": "x"}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
}

#[test]
fn mcp_write_file_missing_content() {
    let r = mcp_tool("write_file", json!({"file_path": "/tmp/x.txt"}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
}

// ── list_dir tool ───────────────────────────────────────────────────

#[test]
fn mcp_list_dir() {
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        tool_call(
            2,
            "shell",
            json!({"command": "mkdir -p /tmp/ld && echo x > /tmp/ld/f.txt && mkdir /tmp/ld/sub"}),
        ),
        tool_call(3, "list_dir", json!({"dir_path": "/tmp/ld"})),
    ]);
    let text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("f.txt"), "text: {text}");
    assert!(text.contains("sub"), "text: {text}");
    assert!(text.contains("dir"), "text: {text}");
    assert!(text.contains("file"), "text: {text}");
}

#[test]
fn mcp_list_dir_missing_path() {
    let r = mcp_tool("list_dir", json!({}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
}

#[test]
fn mcp_list_dir_nonexistent() {
    let r = mcp_tool("list_dir", json!({"dir_path": "/nonexistent"}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
}

// ── unknown tool ────────────────────────────────────────────────────

#[test]
fn mcp_unknown_tool() {
    let r = mcp_tool("nonexistent_tool", json!({}));
    assert!(r["result"]["isError"].as_bool().unwrap_or(false));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("unknown tool"), "text: {text}");
}

// ── ping ────────────────────────────────────────────────────────────

#[test]
fn mcp_ping() {
    let r = mcp_method(2, "ping", json!({}));
    assert!(r["result"].is_object());
}

// ── unknown method (with id → error response) ──────────────────────

#[test]
fn mcp_unknown_method() {
    let r = mcp_method(2, "nonexistent/method", json!({}));
    assert!(r["error"].is_object());
    assert_eq!(r["error"]["code"], -32601);
}

// ── unknown notification (no id → silently skipped) ─────────────────

#[test]
fn mcp_unknown_notification_skipped() {
    let responses = mcp_session(&[
        init_msg(1),
        initialized_msg(),
        // notification (no id) with unknown method — should be skipped
        json!({"jsonrpc": "2.0", "method": "unknown/notification"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    ]);
    // Should get init response + ping response, notification skipped
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[1]["id"], 2);
}

// ── JSON-RPC protocol ───────────────────────────────────────────────

#[test]
fn mcp_jsonrpc_version() {
    let responses = mcp_session(&[init_msg(1)]);
    assert_eq!(responses[0]["jsonrpc"], "2.0");
}

#[test]
fn mcp_response_ids_match() {
    let responses = mcp_session(&[
        json!({"jsonrpc": "2.0", "id": 42, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.1"}
        }}),
        initialized_msg(),
        json!({"jsonrpc": "2.0", "id": 99, "method": "ping"}),
    ]);
    assert_eq!(responses[0]["id"], 42);
    assert_eq!(responses[1]["id"], 99);
}

// ── empty lines and invalid JSON are skipped ────────────────────────

#[test]
fn mcp_empty_lines_skipped() {
    let mut input = String::new();
    input.push('\n'); // empty line
    input.push_str("not valid json\n"); // invalid JSON
    input.push_str(&serde_json::to_string(&init_msg(1)).unwrap());
    input.push('\n');

    let (rt, local) = rt();
    let output = rt.block_on(local.run_until(async {
        let shell = Shell::builder().build().unwrap();
        let kernel = shell.kernel().clone();
        let limits = shell.limits();
        let mut cursor = Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        strands_shell::mcp::serve_io(kernel, &limits, &mut cursor, &mut out).await;
        out
    }));

    let stdout = String::from_utf8(output).unwrap();
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], 1);
}

// ── read_file content-block dispatch ────────────────────────────────
//
// Pre-seed raw bytes via Shell::write_file (covers the binary cases that
// can't round-trip through MCP write_file's text-only `content` parameter),
// then drive serve_io against the same kernel and inspect the content block.

fn mcp_read_with_bytes(path: &str, bytes: Vec<u8>) -> Value {
    let mut input = String::new();
    input.push_str(&serde_json::to_string(&init_msg(1)).unwrap());
    input.push('\n');
    input.push_str(&serde_json::to_string(&initialized_msg()).unwrap());
    input.push('\n');
    input.push_str(
        &serde_json::to_string(&tool_call(2, "read_file", json!({"file_path": path}))).unwrap(),
    );
    input.push('\n');

    let path = path.to_string();
    let (rt, local) = rt();
    let output = rt.block_on(local.run_until(async move {
        let mut shell = Shell::builder().build().unwrap();
        shell.write_file(&path, &bytes).await.unwrap();
        let kernel = shell.kernel().clone();
        let limits = shell.limits();
        let mut cursor = Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        strands_shell::mcp::serve_io(kernel, &limits, &mut cursor, &mut out).await;
        out
    }));

    let stdout = String::from_utf8(output).unwrap();
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    responses[1].clone()
}

#[test]
fn mcp_read_file_image_returns_image_block() {
    // Minimal PNG signature; the mime is chosen from the extension.
    let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    let r = mcp_read_with_bytes("/tmp/pic.png", png_header);
    let block = &r["result"]["content"][0];
    assert_eq!(block["type"], "image");
    assert_eq!(block["mimeType"], "image/png");
    assert!(!block["data"].as_str().unwrap().is_empty());
}

#[test]
fn mcp_read_file_binary_non_image_returns_resource_blob() {
    // Embedded NUL → not UTF-8; unknown extension → octet-stream fallback.
    let bytes = vec![0x00u8, 0xff, 0xfe, 0x42, 0x00, 0x99];
    let r = mcp_read_with_bytes("/tmp/blob.bin", bytes);
    let block = &r["result"]["content"][0];
    assert_eq!(block["type"], "resource");
    let resource = &block["resource"];
    assert_eq!(resource["uri"], "file:///tmp/blob.bin");
    assert_eq!(resource["mimeType"], "application/octet-stream");
    assert!(!resource["blob"].as_str().unwrap().is_empty());
}

#[test]
fn mcp_read_file_pdf_returns_resource_blob_with_pdf_mime() {
    // Lone 0xff/0xfe bytes are invalid UTF-8 → forces the binary path.
    let pdf = b"%PDF-1.4\n%\xff\xfe\x80".to_vec();
    let r = mcp_read_with_bytes("/tmp/doc.pdf", pdf);
    let block = &r["result"]["content"][0];
    assert_eq!(block["type"], "resource");
    assert_eq!(block["resource"]["mimeType"], "application/pdf");
}

#[test]
fn mcp_read_file_markdown_extension_still_text() {
    // `.md` has a known mime but the bytes are valid UTF-8 — text path wins.
    let md = "# title\nhello\n".as_bytes().to_vec();
    let r = mcp_read_with_bytes("/tmp/note.md", md);
    let block = &r["result"]["content"][0];
    assert_eq!(block["type"], "text");
    let text = block["text"].as_str().unwrap();
    assert!(text.contains("# title"));
    assert!(text.contains("hello"));
}

#[test]
fn mcp_read_file_json_with_invalid_utf8_falls_back_to_blob() {
    // Text-ish mime + invalid UTF-8 bytes — the UTF-8 guard wins, so we
    // land on resource/blob with the extension-derived mime.
    let bad = vec![b'{', 0xff, 0xfe, b'}'];
    let r = mcp_read_with_bytes("/tmp/bad.json", bad);
    let block = &r["result"]["content"][0];
    assert_eq!(block["type"], "resource");
    assert_eq!(block["resource"]["mimeType"], "application/json");
}

#[test]
fn mcp_read_file_exceeds_max_output_is_error() {
    // Build a shell with a tiny max_output and seed a file just over it.
    // The MCP read_file call should return isError with the size-limit
    // diagnostic prefixed by the path.
    let path = "/tmp/big.txt";
    let mut input = String::new();
    input.push_str(&serde_json::to_string(&init_msg(1)).unwrap());
    input.push('\n');
    input.push_str(&serde_json::to_string(&initialized_msg()).unwrap());
    input.push('\n');
    input.push_str(
        &serde_json::to_string(&tool_call(2, "read_file", json!({"file_path": path}))).unwrap(),
    );
    input.push('\n');

    let (rt, local) = rt();
    let output = rt.block_on(local.run_until(async move {
        let mut shell = Shell::builder().max_output(64).build().unwrap();
        shell.write_file(path, &vec![b'x'; 1024]).await.unwrap();
        let kernel = shell.kernel().clone();
        let limits = shell.limits();
        let mut cursor = Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        strands_shell::mcp::serve_io(kernel, &limits, &mut cursor, &mut out).await;
        out
    }));

    let stdout = String::from_utf8(output).unwrap();
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let r = &responses[1];
    assert_eq!(r["result"]["isError"], true);
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(path), "error should reference path: {text}");
    assert!(
        text.contains("limit"),
        "error should mention size limit: {text}"
    );
}

// ── McpClient (tests mcp_client.rs via out-of-process) ──────────────

fn shell_bin() -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    path.push("strands-shell");
    path.to_string_lossy().to_string()
}

#[test]
fn mcp_client_start_and_list_tools() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let client =
            strands_shell::mcp_client::McpClient::start(&shell_bin(), &["--mcp".to_string()])
                .await
                .expect("failed to start MCP client");

        assert_eq!(client.tools.len(), 4);
        let names: Vec<&str> = client.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"list_dir"));

        for tool in &client.tools {
            assert!(!tool.description.is_empty());
            assert!(tool.input_schema.is_object());
        }
    });
}

#[test]
fn mcp_client_call_shell() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let client =
            strands_shell::mcp_client::McpClient::start(&shell_bin(), &["--mcp".to_string()])
                .await
                .unwrap();

        let result = client
            .call_tool("shell", json!({"command": "echo hello"}))
            .await
            .unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(text.trim(), "hello");
    });
}

#[test]
fn mcp_client_call_write_and_read() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let client =
            strands_shell::mcp_client::McpClient::start(&shell_bin(), &["--mcp".to_string()])
                .await
                .unwrap();

        client
            .call_tool(
                "write_file",
                json!({
                    "file_path": "/tmp/ct.txt", "content": "from client"
                }),
            )
            .await
            .unwrap();

        let rd = client
            .call_tool("read_file", json!({"file_path": "/tmp/ct.txt"}))
            .await
            .unwrap();
        let text = rd["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("from client"), "text: {text}");
    });
}

#[test]
fn mcp_client_call_list_dir() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let client =
            strands_shell::mcp_client::McpClient::start(&shell_bin(), &["--mcp".to_string()])
                .await
                .unwrap();

        client
            .call_tool(
                "shell",
                json!({"command": "mkdir -p /tmp/cld && echo x > /tmp/cld/a.txt"}),
            )
            .await
            .unwrap();
        let result = client
            .call_tool("list_dir", json!({"dir_path": "/tmp/cld"}))
            .await
            .unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("a.txt"), "text: {text}");
    });
}

#[test]
fn mcp_start_clients() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let entries = vec![strands_shell::mcp_client::McpConfigEntry {
            name: "test-server".to_string(),
            command: shell_bin(),
            args: vec!["--mcp".to_string()],
        }];
        let clients = strands_shell::mcp_client::start_clients(&entries)
            .await
            .unwrap();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].module_name, "test_server");
        assert_eq!(clients[0].client.tools.len(), 4);
    });
}

#[test]
fn mcp_client_bad_command() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let result = strands_shell::mcp_client::McpClient::start("/nonexistent/binary", &[]).await;
        assert!(result.is_err());
    });
}
