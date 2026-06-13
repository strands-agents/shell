use std::io;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

/// A tool discovered from an MCP server.
#[derive(Clone, Debug)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A running MCP server connection.
pub struct McpClient {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    _child: Child,
    next_id: Mutex<u64>,
    pub tools: Vec<McpTool>,
}

impl McpClient {
    /// Spawn an MCP server process, initialize it, and list its tools.
    pub async fn start(command: &str, args: &[String]) -> io::Result<Self> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;

        let mut client = Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            _child: child,
            next_id: Mutex::new(1),
            tools: Vec::new(),
        };

        // Initialize
        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "strands-shell", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;

        // Send initialized notification (no response expected)
        client
            .notify("notifications/initialized", json!({}))
            .await?;

        // List tools
        let result = client.request("tools/list", json!({})).await?;
        if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
            for tool in tools {
                let name = tool
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let description = tool
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string();
                let input_schema = tool.get("inputSchema").cloned().unwrap_or(json!({}));
                client.tools.push(McpTool {
                    name,
                    description,
                    input_schema,
                });
            }
        }

        Ok(client)
    }

    async fn send(&self, msg: &Value) -> io::Result<()> {
        let mut stdin = self.stdin.lock().await;
        let line = serde_json::to_string(msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await
    }

    async fn read_response(&self) -> io::Result<Value> {
        let mut stdout = self.stdout.lock().await;
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "MCP server closed",
                ));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: Value = serde_json::from_str(trimmed)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            // Skip notifications (no id)
            if msg.get("id").is_some() {
                return Ok(msg);
            }
        }
    }

    async fn request(&self, method: &str, params: Value) -> io::Result<Value> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        let resp = self.read_response().await?;
        if let Some(err) = resp.get("error") {
            return Err(io::Error::other(
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("MCP error"),
            ));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    /// Call a tool by name with the given arguments object.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> io::Result<Value> {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        )
        .await
    }
}

/// Named MCP client with its module name.
pub struct NamedMcpClient {
    pub module_name: String,
    pub client: McpClient,
}

/// Start all MCP clients from config entries.
pub async fn start_clients(entries: &[McpConfigEntry]) -> io::Result<Vec<NamedMcpClient>> {
    let mut clients = Vec::new();
    for entry in entries {
        let client = McpClient::start(&entry.command, &entry.args).await?;
        // Convert name to valid Lua module name (replace - with _)
        let module_name = entry.name.replace('-', "_");
        clients.push(NamedMcpClient {
            module_name,
            client,
        });
    }
    Ok(clients)
}

/// Config entry for an MCP server (parsed from TOML).
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfigEntry {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}
