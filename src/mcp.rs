use std::io::{self, BufRead, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::exec;
use crate::os::{self, Kernel, Process, ProcessLimits};

// ── JSON-RPC types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct Request {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

fn ok(id: Value, result: Value) -> Response {
    Response {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Value, code: i32, msg: &str) -> Response {
    Response {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(json!({"code": code, "message": msg})),
    }
}

fn text_result(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

fn err_result(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

/// MCP `image` content block. Used when `read_file` detects an image
/// extension; bytes are base64'd, mime is the extension-derived value.
fn image_result(bytes: &[u8], mime: &str) -> Value {
    json!({"content": [{
        "type": "image",
        "data": os::base64_encode(bytes),
        "mimeType": mime,
    }]})
}

/// MCP `resource` content block with a base64 `blob`. Used for non-image
/// binary payloads (PDF, archives, anything that isn't valid UTF-8). The
/// `uri` exposes the VFS path so hosts can correlate it with later
/// `resources/read` calls if they want. Always emits `file:///{path}` —
/// trims any leading slashes from the input so a relative path doesn't
/// land in the URI's authority slot per RFC 8089.
fn blob_resource_result(path: &str, bytes: &[u8], mime: &str) -> Value {
    let stripped = path.trim_start_matches('/');
    json!({"content": [{
        "type": "resource",
        "resource": {
            "uri": format!("file:///{stripped}"),
            "mimeType": mime,
            "blob": os::base64_encode(bytes),
        }
    }]})
}

/// Map a path's extension to a MIME type for the cases agents touch most.
/// Returns `None` when we don't recognize the extension; callers then fall
/// back to UTF-8 sniffing for the text/binary split. Scoped to the basename
/// via `Path::extension` so a bare `"png"` or a path like `/a.b/c` doesn't
/// accidentally match.
fn mime_from_extension(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        "xml" => "application/xml",
        _ => return None,
    };
    Some(mime)
}

// ── Tool definitions ────────────────────────────────────────────────

fn tool_list() -> Value {
    json!({"tools": [
        {
            "name": "shell",
            "description": "Runs a command in the strands-shell virtual shell. Returns two text content blocks: content[0].text is stdout, content[1].text is stderr (both always present, empty string when a stream produced nothing). The exit code is in metadata.exit_code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command string to execute."
                    },
                    "timeout_ms": {
                        "type": "number",
                        "description": "Timeout in milliseconds (default: 30000)."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_file",
            "description": "Reads a file from the virtual filesystem. Text files return as line-numbered text (1-indexed, honors offset/limit). Images return as image content; other binary files return as embedded resource blobs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file."
                    },
                    "offset": {
                        "type": "number",
                        "description": "1-indexed line number to start reading from (default: 1)."
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of lines to return (default: 2000)."
                    }
                },
                "required": ["file_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "write_file",
            "description": "Creates or overwrites a file in the virtual filesystem.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file."
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file."
                    }
                },
                "required": ["file_path", "content"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_dir",
            "description": "Lists entries in a directory in the virtual filesystem.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dir_path": {
                        "type": "string",
                        "description": "Absolute path to the directory."
                    }
                },
                "required": ["dir_path"],
                "additionalProperties": false
            }
        }
    ]})
}

// ── Tool execution ──────────────────────────────────────────────────

async fn exec_shell(kernel: &Arc<dyn Kernel>, proc: &mut Process, args: &Value) -> Value {
    let command = match args.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return err_result("missing required parameter: command"),
    };
    let max_timeout_ms = proc
        .deadline
        .map(|dl| {
            dl.saturating_duration_since(tokio::time::Instant::now())
                .as_millis() as u64
        })
        .unwrap_or(30_000);
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(30_000)
        .min(max_timeout_ms);

    // proc.deadline is re-armed by apply_limits at the start of every
    // tools/call, so we don't bother saving/restoring across this call.
    proc.deadline =
        Some(tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms));

    let (exit_code, stdout, stderr) = exec::execute_capture(kernel.clone(), proc, command).await;

    // Two-block result: content[0] is always stdout, content[1] is always
    // stderr. Both blocks are always present (an empty stream is "text": "")
    // so agents can reason about each stream independently without parsing a
    // concatenated buffer. The exit code lives on metadata.exit_code.
    json!({
        "content": [
            {"type": "text", "text": stdout},
            {"type": "text", "text": stderr}
        ],
        "metadata": {"exit_code": exit_code}
    })
}

async fn exec_read_file(kernel: &Arc<dyn Kernel>, proc: &mut Process, args: &Value) -> Value {
    let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err_result("missing required parameter: file_path"),
    };
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

    let flags = crate::os::OpenFlags::read();
    let fd = match kernel.open(proc, file_path, flags).await {
        Ok(fd) => fd,
        Err(e) => return err_result(&format!("failed to open {file_path}: {e}")),
    };

    let mut reader = match proc.take_reader(fd) {
        Ok(r) => r,
        Err(e) => return err_result(&format!("failed to read {file_path}: {e}")),
    };

    // Read raw bytes once; downstream branches decide on a presentation.
    let bytes = match os::read_to_end_limited(&mut reader, proc.max_output).await {
        Ok(b) => b,
        Err(e) => return err_result(&format!("failed to read {file_path}: {e}")),
    };

    // Dispatch by mime: image/* → image block; non-text → resource/blob;
    // valid UTF-8 → the historical line-numbered text presentation. Text
    // wins when bytes are valid UTF-8 even if the extension says otherwise,
    // so a `.json` blob with a stray binary byte still surfaces as a
    // resource rather than corrupting the text channel.
    let mime = mime_from_extension(file_path);
    if let Some(m) = mime
        && m.starts_with("image/")
    {
        return image_result(&bytes, m);
    }

    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(_) => {
            let m = mime.unwrap_or("application/octet-stream");
            return blob_resource_result(file_path, &bytes, m);
        }
    };

    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = (offset - 1).min(total);
    let end = (start + limit).min(total);
    let selected = &lines[start..end];

    let mut result = String::new();
    for (i, line) in selected.iter().enumerate() {
        let line_num = start + i + 1;
        result.push_str(&format!("{line_num:>6}\t{line}\n"));
    }

    if end < total {
        result.push_str(&format!(
            "\n... ({} more lines, {} total)\n",
            total - end,
            total
        ));
    }

    text_result(&result)
}

async fn exec_write_file(kernel: &Arc<dyn Kernel>, proc: &mut Process, args: &Value) -> Value {
    let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err_result("missing required parameter: file_path"),
    };
    let content = match args.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return err_result("missing required parameter: content"),
    };

    let flags = crate::os::OpenFlags::write();
    let fd = match kernel.open(proc, file_path, flags).await {
        Ok(fd) => fd,
        Err(e) => return err_result(&format!("failed to create {file_path}: {e}")),
    };

    let mut writer = match proc.take_writer(fd) {
        Ok(w) => w,
        Err(e) => return err_result(&format!("failed to write {file_path}: {e}")),
    };

    use tokio::io::AsyncWriteExt;
    if let Err(e) = writer.write_all(content.as_bytes()).await {
        return err_result(&format!("write error: {e}"));
    }
    drop(writer);

    // Yield to let the VFS background flush task complete
    tokio::task::yield_now().await;

    text_result(&format!("Wrote {} bytes to {file_path}", content.len()))
}

async fn exec_list_dir(kernel: &Arc<dyn Kernel>, proc: &mut Process, args: &Value) -> Value {
    let dir_path = match args.get("dir_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err_result("missing required parameter: dir_path"),
    };

    let entries = match kernel.list_dir(proc, dir_path).await {
        Ok(e) => e,
        Err(e) => return err_result(&format!("failed to list {dir_path}: {e}")),
    };

    let mut result = String::new();
    for entry in &entries {
        let kind = if entry.is_dir { "dir" } else { "file" };
        result.push_str(&format!("{}\t{}\n", kind, entry.name));
    }

    text_result(&result)
}

// ── Server loop ─────────────────────────────────────────────────────

/// Process MCP JSON-RPC messages from a reader, writing responses to a writer.
/// This is the core server loop, factored out for testability.
pub async fn serve_io(
    kernel: Arc<dyn Kernel>,
    limits: &ProcessLimits,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) {
    // Session-scoped Process: cwd, env, exported vars, shell functions, and
    // open fds persist across tools/call for the lifetime of this connection.
    // The kernel (VFS, mounts, credentials) is already shared by construction.
    // Per-call limits (including the deadline) are re-armed inside the loop.
    let mut session_proc = kernel.new_process();

    for line in input.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let id = req.id.clone().unwrap_or(Value::Null);

        let resp = match req.method.as_str() {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "strands-shell", "version": env!("CARGO_PKG_VERSION")}
                }),
            ),
            "notifications/initialized" => continue, // notification, no response
            "tools/list" => ok(id, tool_list()),
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));

                // Re-arm per-call limits (notably the deadline) without dropping
                // session state like cwd, env, functions, or open fds.
                session_proc.apply_limits(limits);

                let result = match name {
                    "shell" => exec_shell(&kernel, &mut session_proc, &args).await,
                    "read_file" => exec_read_file(&kernel, &mut session_proc, &args).await,
                    "write_file" => exec_write_file(&kernel, &mut session_proc, &args).await,
                    "list_dir" => exec_list_dir(&kernel, &mut session_proc, &args).await,
                    _ => err_result(&format!("unknown tool: {name}")),
                };
                ok(id, result)
            }
            "ping" => ok(id, json!({})),
            _ => {
                // Unknown method — skip notifications (no id), error for requests
                if req.id.is_some() {
                    err(id, -32601, &format!("method not found: {}", req.method))
                } else {
                    continue;
                }
            }
        };

        let json = serde_json::to_string(&resp).expect("serialize response");
        let _ = writeln!(output, "{json}");
        let _ = output.flush();
    }
}

pub async fn serve(kernel: Arc<dyn Kernel>, limits: ProcessLimits) {
    let stdin = io::stdin();
    let mut locked = stdin.lock();
    let mut stdout = io::stdout();
    serve_io(kernel, &limits, &mut locked, &mut stdout).await;
}
