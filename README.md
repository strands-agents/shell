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
    A virtual shell for AI agents that runs entirely in-process.
  </h2>

  <div align="center">
    <a href="https://python.org"><img alt="Python" src="https://img.shields.io/badge/Python-3.10%2B-blue?logo=python"/></a>
    <a href="https://nodejs.org"><img alt="Node" src="https://img.shields.io/badge/Node-18%2B-green?logo=nodedotjs"/></a>
    <a href="#license"><img alt="License" src="https://img.shields.io/badge/License-Apache_2.0-blue"/></a>
    <a href="https://discord.gg/strands"><img alt="Strands Discord" src="https://img.shields.io/badge/Discord-Strands-5865F2?logo=discord&logoColor=white"/></a>
  </div>

  <p>
    <a href="https://strandsagents.com/">Documentation</a>
    ◆ <a href="#python">Python</a>
    ◆ <a href="#nodejs">Node.js</a>
  </p>
</div>

Strands Shell is a virtual shell that runs entirely inside a single userspace
process. It supports Bourne-compatible syntax and provides the commands an AI
agent needs, but never calls fork/exec or makes direct system calls. Every
operation flows through a `Kernel` mediation boundary, giving you fine-grained
control over what the agent can see and do — down to individual files, network
domains, or credentials — without containers, microVMs, or firewalls.

## Quick Start

```bash
pip install strands-shell
```

```python
import strands_shell

shell = strands_shell.Shell(binds=[strands_shell.Bind("/path/to/project", "/workspace", mode="copy")])
out = shell.run("grep -rn TODO /workspace")
print(out.stdout)
```

State persists across `run()` calls (env vars, working directory, functions);
the filesystem is shared. Native bindings exist for [Python](#python) and
[Node.js](#nodejs).

## Configuration

```python
shell = strands_shell.Shell(
    # Filesystem — copy-mode is an isolated snapshot, direct passes through
    binds=[
        strands_shell.Bind("/host/project", "/workspace", mode="copy"),
        strands_shell.Bind("/tmp/output", "/output", mode="direct"),
    ],
    # HTTP credentials, injected by URL prefix at request time
    credentials=[strands_shell.Cred("https://api.example.com/", env_var="API_TOKEN")],
    # Behavioral settings
    timeout=30.0,                       # per-command wall-clock seconds
    env={"PROJECT": "demo"},
    # Resource limits (namespaced)
    limits=strands_shell.Limits(
        max_output=1 << 20,             # 1 MB stdout cap
        max_file_size=10 << 20,         # 10 MB per file
    ),
)
```

| `Bind` argument                       | Behavior                                          |
|---------------------------------------|---------------------------------------------------|
| `mode="copy"`                         | Copy files into the VFS at build time (snapshot)  |
| `mode="direct"`                       | Pass reads/writes through to the host filesystem  |
| `readonly=True`                       | Reject writes through the mount (either mode)     |

Or load it all from TOML with `config_file=`:

```toml
[[bind]]
mode = "direct"
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

## Filesystem Operations

Read and write files directly without spawning a shell:

```python
shell.write_file("/workspace/note.txt", b"hello")
data = shell.read_file("/workspace/note.txt")
entries = shell.list_files("/workspace")
shell.remove_file("/workspace/note.txt")
```

## Security Model

Strands Shell is a strong **in-process mediation layer**, not a hardened
sandbox. The Kernel boundary is real, but the shell runs in the host's
address space; for workloads that require VM- or container-level isolation,
run Strands Shell inside one. Treat the controls below as defense-in-depth.

**What we protect against (escapes from Kernel mediation):**

- File access beyond explicitly bound paths
- Arbitrary system calls — Strands Shell never calls `fork`/`exec` or makes
  direct syscalls; all operations are mediated by the `Kernel` implementation
- Local privilege escalation through the shell environment
- SSRF and metadata-service access — `curl` and `http_request` block
  RFC1918, link-local, loopback, and IMDS/ECS-task-role addresses at DNS
  resolution time

A bypass of any of these controls is treated as a security issue. Please
report responsibly (see [Security](#security)).

**Best-effort:** resource limits (timeouts, output caps, fd limits, inode
limits, pipeline depth) catch runaway agents but won't withstand a
determined adversary. Use OS-level isolation for hard CPU/memory bounds.

**Not protected:** speculative side channels (Spectre, etc.), and
multi-tenancy within a single process. Use separate processes or VMs for
strong tenant isolation.

## MCP Server

Strands Shell ships a built-in [Model Context Protocol](https://modelcontextprotocol.io/)
server, exposing the shell to any MCP-compatible AI client as tools (`shell`,
`read_file`, `write_file`, `list_dir`) over JSON-RPC on stdio.

Installing the Python package puts a `strands-shell` launcher on your PATH, so
the server runs with no separate download. Point your MCP client at it:

```json
{
  "mcpServers": {
    "strands-shell": {
      "command": "uvx",
      "args": ["strands-shell", "--config", "/path/to/sandbox.toml", "--mcp"]
    }
  }
}
```

`uvx` fetches and runs Strands Shell on demand — nothing to install first. If
you'd rather install it into the active environment, `pip install strands-shell`
and use `"command": "strands-shell", "args": ["--config", "/path/to/sandbox.toml", "--mcp"]`
instead. The `--config` TOML declares the bind mounts, credentials, and limits
the agent runs under (see [Configuration](#configuration)); drop it to serve a
bare in-memory sandbox.

You can also run it directly to try it out:

```sh
uvx strands-shell --mcp                          # bare in-memory sandbox
uvx strands-shell --config sandbox.toml --mcp    # with mounts/credentials
```

MCP **client** servers configured under `[[mcp]]` in your TOML are also
auto-exposed as Lua modules — `require("my_tools")` returns a table of the
server's tools.

## Python

```sh
pip install strands-shell
```

```python
import strands_shell

shell = strands_shell.Shell(timeout=30.0)
out = shell.run("echo hello | tr a-z A-Z")
print(out.stdout)  # HELLO
```

## Node.js

```sh
npm install @strands-agents/shell
```

```javascript
import { Shell } from '@strands-agents/shell'

const shell = await Shell.create({ timeout: 30.0 })
const out = await shell.run('echo hello | tr a-z A-Z')
console.log(out.stdout)  // HELLO
```

All methods return Promises; bytes use `Uint8Array` (Node `Buffer` works
unchanged).

## WebAssembly (WASM)

Strands Shell compiles to a `wasm32-wasip2` module, so the shell can run inside
any WASI runtime with each instance isolated in its own linear memory. Build it
with [wasi-sdk](https://github.com/WebAssembly/wasi-sdk) 32+ and run it through a
WASI host such as [wasmtime](https://wasmtime.dev/):

```sh
./scripts/build-wasm.sh --release
echo 'echo hello from wasm' | wasmtime -W exceptions=y -S http strands-shell-wasm.wasm
```

The WASM build is a reduced surface: it reads commands from stdin and writes to
stdout/stderr, with no PyO3/Node bindings, no built-in MCP server, and no
`--config` file. `curl` requires the host to grant outbound HTTP (the `-S http`
flag above).

## Supported Commands

### File Operations
`cat`, `cp`, `chmod`, `head`, `ln`, `ls`, `mkdir`, `mktemp`, `mv`, `rm`,
`rmdir`, `tail`, `tee`, `touch`

### Text Processing
`cut`, `grep`, `jq`, `sed`, `sort`, `tr`, `uniq`, `wc`

### Search
`find`, `xargs`

### Networking
`curl` — SSRF-guarded, with automatic credential injection

### Path Utilities
`basename`, `dirname`, `readlink`

### Other
`date`, `echo`, `env`, `false`, `pwd`, `sleep`, `true`

### Scripting
`lua` — embedded Lua 5.4 interpreter with interactive REPL (`lua -i`)

### Shell Builtins
`alias`, `cd`, `eval`, `exec`, `exit`, `export`, `getopts`, `hash`,
`local`, `printf`, `read`, `readonly`, `return`, `set`, `shift`, `source`
(`.`), `test` (`[`), `trap`, `type`, `umask`, `unalias`, `unset`, `wait`

### Shell Features

Pipelines, redirections (`>`, `>>`, `<`, `2>`, `&>`, here-documents `<<`),
conditionals (`if`/`elif`/`else`, `&&`, `||`), loops (`for`, `while`,
`until`), case statements, command groups and subshells, functions,
variable expansion (`$VAR`, `${VAR:-default}`, `${VAR%pattern}`,
`${#VAR}`), command substitution (`` `cmd` `` and `$(cmd)`), arithmetic
expansion, glob expansion, single/double quoting, background jobs (`&`),
script execution (`. script.sh`, `source`), aliases, local variables,
`set -e`/`-u`/`-x`, readonly variables.

## Documentation

- [Strands Agents Documentation](https://strandsagents.com/) — the broader SDK Strands Shell plugs into, including the Strands Shell guides

## Contributing ❤️

Bug reports, design feedback, and PRs are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) to get started.

## Stay in touch with the team

Come meet the Strands team and other users on
[**Discord**](https://discord.com/invite/strands).

## License

This project is licensed under the Apache License 2.0.

## Security

If you discover a security issue, please report it responsibly rather than
opening a public issue. Bypasses of filesystem mediation, SSRF protection,
or credential injection are treated as security issues.
