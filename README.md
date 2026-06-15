<div align="center">
  <div>
    <a href="https://strandsagents.com">
      <img src="https://strandsagents.com/latest/assets/logo-github.svg" alt="Strands Agents" width="55px" height="105px">
    </a>
  </div>

  <h1>
    Strands Shell
  </h1>

  <h2>
    Give your agent a shell without giving it the keys to your machine.
  </h2>

  <div align="center">
    <a href="https://python.org"><img alt="Python" src="https://img.shields.io/badge/Python-3.10%2B-blue?logo=python"/></a>
    <a href="https://nodejs.org"><img alt="Node" src="https://img.shields.io/badge/Node-18%2B-green?logo=nodedotjs"/></a>
    <a href="https://crates.io/crates/strands-shell"><img alt="crates.io" src="https://img.shields.io/crates/v/strands-shell"/></a>
    <a href="#license"><img alt="License" src="https://img.shields.io/badge/License-Apache_2.0-blue"/></a>
    <a href="https://discord.gg/strands"><img alt="Strands Discord" src="https://img.shields.io/badge/Discord-Strands-5865F2?logo=discord&logoColor=white"/></a>
  </div>

  <p>
    <a href="https://strandsagents.com/">Documentation</a>
    ◆ <a href="#mcp-server">MCP Server</a>
    ◆ <a href="#python">Python</a>
    ◆ <a href="#nodejs">Node.js</a>
    ◆ <a href="#rust">Rust</a>
  </p>
</div>

---

Agents run shell commands in tight loops. Install deps, run tests, grep for errors, iterate. Those loops need speed and isolation. 

Strands Shell is a Bourne-compatible shell that runs in-process. `grep`, `sed`, `jq`, `curl`, `find`, 50+ commands. It does this without fork, exec, syscalls, or cold starts. You declare what the agent can reach (files, URLs, credentials) and everything else doesn't exist to the agent.

| | Docker | Cloud sandbox | Strands Shell |
|---|---|---|---|
| **Cold start** | ~200ms | ~1s (network) | <1ms |
| **Isolation** | Container namespace | MicroVM | In-process VFS |
| **Network** | iptables / sidecar | Platform policy | URL allowlist + SSRF guard |
| **Secrets** | Env vars (agent can read them) | Platform-specific | Injected per-request, agent never sees them |
| **Setup** | Docker daemon | API key + network | `pip install strands-shell` |
| **Platforms** | Linux | Cloud-only | macOS, Linux, WASM |

## Quick Start

### MCP (works with any agent framework)

Drop this into your MCP client config:

```json
{
  "mcpServers": {
    "shell": {
      "command": "uvx",
      "args": ["strands-shell", "--mcp"]
    }
  }
}
```

That's it. Your agent gets `shell`, `read_file`, `write_file`, `list_dir`. All mediated through the Kernel. `uvx` handles the install.

### Python

```bash
pip install strands-shell
```

```python
import strands_shell

shell = strands_shell.Shell(
    binds=[strands_shell.Bind("/my/project", "/workspace", mode="copy")],
    credentials=[strands_shell.Cred("https://api.example.com/", env_var="API_TOKEN")],
    allowed_urls=["https://api.example.com/"],
)

out = shell.run("grep -rn TODO /workspace")
print(out.stdout)
```

### Node.js

```bash
npm install @strands-agents/shell
```

```javascript
import { Shell } from '@strands-agents/shell'

const shell = await Shell.create({
  binds: [{ source: '/my/project', destination: '/workspace', mode: 'copy' }],
})
const out = await shell.run('grep -rn TODO /workspace')
console.log(out.stdout)
```

## How It Works

```
┌────────────────────────────────────────────┐
│             Your agent code                │
│   (Strands, LangGraph, Pydantic AI, etc)   │
└──────────────────┬─────────────────────────┘
                   │ MCP / Python / Node.js
┌──────────────────▼─────────────────────────┐
│            Strands Shell                   │
│                                            │
│  ┌─────────────────────────────────────┐   │
│  │  Kernel (mediation boundary)        │   │
│  │  • VFS: isolated virtual filesystem │   │
│  │  • Network: SSRF guard + allowlist  │   │
│  │  • Credentials: injected per-URL    │   │
│  │  • Limits: timeout, output, fds     │   │
│  └─────────────────────────────────────┘   │
│                                            │
│  Shell engine: parser, builtins, commands  │
│  25 builtins + 33 commands + Lua 5.4       │
└────────────────────────────────────────────┘
```

It's a Rust crate that compiles to native (macOS/Linux), Python via PyO3, Node.js via napi-rs, and WASM (wasi-p2). State persists across `run()` calls (env vars, working directory, functions). The filesystem is shared.

## Configuration

```python
shell = strands_shell.Shell(
    binds=[
        strands_shell.Bind("/host/project", "/workspace", mode="copy"),
        strands_shell.Bind("/tmp/output", "/output", mode="direct"),
    ],
    credentials=[
        strands_shell.Cred("https://api.example.com/", env_var="API_TOKEN"),
    ],
    allowed_urls=["https://api.example.com/", "https://pypi.org/"],
    timeout=30.0,
    env={"PROJECT": "demo"},
    limits=strands_shell.Limits(
        max_output=1 << 20,
        max_file_size=10 << 20,
    ),
)
```

| Bind mode | What happens |
|-----------|----------|
| `mode="copy"` | Snapshot host files into VFS at build time. Agent works on a copy. |
| `mode="direct"` | Reads and writes pass through to the host filesystem. |
| `readonly=True` | Writes rejected. Works with either mode. |

### TOML

You can load all of this from a config file instead:

```toml
[[bind]]
mode = "copy"
source = "/host/project"
destination = "/workspace"

[[cred]]
url = "https://api.openai.com/v1/"
methods = ["POST"]
kind = "bearer"
api_key_env = "OPENAI_API_KEY"

[[mcp]]
name = "my-tools"
command = "/path/to/mcp-server"
args = ["--stdio"]
```

## MCP Server

The built-in [MCP](https://modelcontextprotocol.io/) server exposes the shell over JSON-RPC on stdio. Works with anything that speaks MCP.

```sh
uvx strands-shell --mcp                          # bare in-memory sandbox
uvx strands-shell --config sandbox.toml --mcp    # with mounts + credentials
```

If you declare `[[mcp]]` servers in your TOML config, they show up as Lua modules inside the shell. Call `require("my_tools")` and you get a table of the server's tools.

## Security Model

The Kernel mediates everything. It runs in the same process as your code, not in a VM. If your threat model is "untrusted tenant running arbitrary code," put Strands Shell inside a container too. For "my agent shouldn't access things I haven't explicitly allowed," the Kernel handles it.

**Default-deny. You allowlist what the agent can reach:**

- Files: only bound paths exist. Everything else is gone.
- Network: `curl` blocks private ranges (RFC1918, link-local, loopback, IMDS) by default. Public URLs pass through. Use `allowed_urls` to permit specific internal hosts.
- Secrets: the Kernel injects credentials per-URL at request time. The agent never holds them. The Kernel never re-injects on redirects, even back to the same host.
- Syscalls: there are none. No `fork`, no `exec`. The shell is pure userspace.

If you bypass any of these, report it. See [SECURITY.md](SECURITY.md).

**Limits (best-effort):** timeouts, output caps, fd limits, inode limits. These catch runaway agents but won't stop someone actively trying to break out. OS-level isolation for that.

**Multi-tenant:** a Shell instance is single-owner. If you're serving multiple agents, create one Shell per session. Construction is cheap (no containers, no VMs, just an in-memory VFS), so spinning up per-request is the intended pattern.

## Commands

### File Operations
`cat` `cp` `chmod` `head` `ln` `ls` `mkdir` `mktemp` `mv` `rm` `rmdir` `tail` `tee` `touch`

### Text Processing
`cut` `grep` `jq` `sed` `sort` `tr` `uniq` `wc`

### Search
`find` `xargs`

### Networking
`curl` with SSRF protection and automatic credential injection

### Path Utilities
`basename` `dirname` `readlink`

### Other
`date` `echo` `env` `false` `pwd` `sleep` `true`

### Scripting
`lua` (embedded Lua 5.4, interactive REPL with `lua -i`)

### Shell Builtins
`alias` `cd` `eval` `exec` `exit` `export` `getopts` `hash` `local` `printf` `read` `readonly` `return` `set` `shift` `source` `test` `trap` `type` `umask` `unalias` `unset` `wait`

### Shell Features
Pipelines, redirections, here-documents, conditionals (`if`/`elif`/`else`, `&&`, `||`), loops (`for`, `while`, `until`), case statements, subshells, functions, variable expansion (`${VAR:-default}`, `${VAR%pattern}`), command substitution, arithmetic, globs, quoting, background jobs, `set -eux`.

## File Operations API

Read and write files without going through a shell command:

```python
shell.write_file("/workspace/note.txt", b"hello")
data = shell.read_file("/workspace/note.txt")
entries = shell.list_files("/workspace")
shell.remove_file("/workspace/note.txt")
```

## Rust

```rust
use strands_shell::{Shell, Bind, BindMode};

let shell = Shell::builder()
    .bind(Bind::new("/host/project", "/workspace", BindMode::Copy))
    .timeout(30)
    .build()
    .unwrap();

let out = shell.run("grep -rn TODO /workspace").await;
println!("{}", out.stdout);
```

## WASM

Compiles to `wasm32-wasip2`. Run it in any WASI runtime:

```sh
./scripts/build-wasm.sh --release
echo 'echo hello' | wasmtime -W exceptions=y -S http strands-shell-wasm.wasm
```

The WASM build is stripped down: stdin/stdout only, no bindings, no MCP server. `curl` works if the host grants outbound HTTP.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports and design questions are just as useful as PRs.

## Community

[Discord](https://discord.com/invite/strands) if you want to talk about it.

## License

Apache-2.0

## Security

If you find a security issue, report it privately instead of opening a public issue. Bypasses of filesystem mediation, SSRF protection, or credential injection qualify. See [SECURITY.md](SECURITY.md).
