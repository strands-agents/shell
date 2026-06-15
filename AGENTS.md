# AGENTS.md

This document provides context, patterns, and guidelines for AI coding assistants working in this repository. For human contributors, see [CONTRIBUTING.md](./CONTRIBUTING.md).

## Working with the Community

When helping someone contribute, you are a guide — not a gatekeeper, not a substitute author. The contribution is theirs; help them make it good and learn along the way. The standard for what makes a good contribution lives in [CONTRIBUTING.md](./CONTRIBUTING.md#using-ai-tools); this is about the people.

- **Point people to the community.** Real questions and design discussion belong with people — the [Discord](https://discord.gg/strands) and [GitHub Discussions](https://github.com/strands-agents/shell/discussions).
- **Assume good faith.** Most contributors are learning; meet them where they are. Good first issues are for bringing newcomers in, not just tickets to close.
- **Talk with contributors, not at them.** Warm, plain, concise. One question at a time, no walls of text, never patronizing. Explain the *why* so it teaches rather than dictates.

## Product Overview

**Strands Shell** is a Bourne-compatible shell for AI agents that runs entirely in-process. It implements a complete operating-system environment inside a single userspace process — inspired by BusyBox and Toybox — but it never calls `fork`/`exec` or makes direct system calls. Every operation flows through a pluggable `Kernel` trait, giving callers fine-grained control over what an agent can see and do (files, network domains, credentials) without containers, microVMs, or firewalls.

It is a Rust crate that compiles to several targets from one source of truth:

- **Native binary** (`strands-shell`) and a Rust library
- **Python** extension module (`strands-shell` on PyPI), via [PyO3](https://pyo3.rs/) + [maturin](https://www.maturin.rs/)
- **Node.js** native addon (`@strands-agents/shell` on npm), via [napi-rs](https://napi.rs/)
- **WASM** module targeting `wasm32-wasip2`

## Architecture

The crate is the single source of truth; every binding wraps the same core.

- **`Kernel` trait (`src/os.rs`) is the security boundary.** All filesystem, process, and network effects go through a `Kernel` implementation. There is no `fork`/`exec` and no direct syscalls. The bundled implementation is `VfsKernel` (`src/vfs_kernel.rs`), backed by an in-process virtual filesystem (`src/vfs.rs`); callers can supply their own (S3-backed, database-backed, etc.) via `Shell::with_kernel()`.
- **In-process VFS with binds.** Directories are mounted into the VFS as *binds* — `copy` mode snapshots files into the VFS at build time; `direct` mode passes reads/writes through to the host, mediated by the kernel. Bind configuration lives in `src/vfs_config.rs`.
- **Commands are split in two:**
  - **Builtins** in `src/builtins/` — things that mutate shell state (`cd`, `export`, `alias`, `set`, control-flow helpers, etc.). They are dispatched by name in `src/builtins/mod.rs` (`lookup()` matches the builtin name to its function).
  - **Isolated commands** in `src/commands/` — coreutils-style programs (`cat`, `grep`, `sed`, `curl`, `jq`, …) that take a kernel + args and produce output. Each is registered with the `#[command("name")]` proc-macro from `strands-shell-macros`.
- **Parser / executor:** `src/parser.rs` parses shell syntax; `src/exec.rs` evaluates it (pipelines, redirections, expansions, control flow).
- **Bindings:** `src/python.rs` (PyO3, the `strands_shell._native` module) and `src/js.rs` (napi-rs). They are intentionally parallel in shape — keep them in sync semantically. The customer-facing Python surface (the `Shell`, `Bind`, `Cred`, `Limits` classes and typed errors) lives in the pure-Python wrapper at `python/strands_shell/__init__.py`.
- **WASM entry:** `src/wasm_main.rs` reads commands from WASI stdin and writes to WASI stdout/stderr; each instance runs in isolated linear memory.
- **MCP:** `src/mcp.rs` is the built-in MCP *server* (exposes the shell as tools); `src/mcp_client.rs` is the MCP *client* (servers configured under `[[mcp]]` become Lua modules).

### The `#[command(...)]` macro

A new isolated command is a function annotated with the proc-macro:

```rust
#[command("ls")]
async fn cmd_ls(os: &dyn Kernel, args: &[String]) -> i32 {
    // ...
}
```

The macro (defined in `strands-shell-macros/src/lib.rs`) registers the command via `inventory` on native targets and feeds the static lookup table used on WASM. Builtins are *not* registered this way — add them to the `match` in `src/builtins/mod.rs`.

## Build & Test Commands

Use the same commands as [CONTRIBUTING.md](./CONTRIBUTING.md#development-environment) so they don't drift. The Rust toolchain is required for every workflow because all bindings build from the crate.

### Rust (shell core)

```bash
cargo build                              # build the library and binaries
cargo test --workspace --all-targets     # unit + integration tests
cargo fmt                                # format
cargo clippy --workspace --all-targets   # lint
cargo doc --workspace --no-deps --open   # API reference
```

Integration tests live in `tests/`: `shell_integration.rs`, `curl_integration.rs`, `lua_integration.rs`, `mcp_integration.rs`, `vfs_unit.rs`.

### Python bindings

```bash
python -m venv .venv && source .venv/bin/activate
pip install maturin pytest
maturin develop --features python        # build + install into the venv
pytest tests/python -v                   # run the Python test suite
```

Python sources are under `python/strands_shell/`; the compiled module is `strands_shell._native`. Tests: `tests/python/*.py`.

### Node.js bindings

```bash
npm install            # install dependencies
npm run build          # release build of the native addon
npm run build:debug    # faster debug build for local development
npm test               # run the Node.js test suite (tests/js/*.mjs)
```

### WASM module

```bash
./scripts/build-wasm.sh --release        # needs wasi-sdk >= 32
```

See [CONTRIBUTING.md](./CONTRIBUTING.md#wasm-module) for which features are available under WASM (no PyO3, no MCP server, no `--config`). WASM is a build target, not a published release artifact.

### CI merge gate

`.github/workflows/ci.yml` runs the Rust suite (`cargo test --workspace --all-targets` + `cargo doc` with `-D warnings`) across Linux/macOS, the Python matrix (`maturin develop --release` + `pytest tests/python`), the Node matrix (`npm run build:debug` + `npm test`), and a security-audit job. Don't open a PR with known failures in the bindings you touched.

## Key Conventions

### Rename scope — the `lash` persona is intentional, do NOT rename it

The identifiers `lash`, `/bin/lash`, `USER=lash`, `/home/lash`, `LASH_UID`, and `LASH_GID` are an **intentional emulated-POSIX persona**. They define the *simulated* Unix environment the shell presents to commands and scripts — the default user, home directory, and uids/gids inside the VFS — not the product name. **Do not "fix" them to `strands-shell`.** They appear by design in `src/os.rs`, `src/vfs.rs`, `src/vfs_config.rs`, and `src/vfs_kernel.rs`. Renaming them changes the emulated environment and breaks tests and scripts that expect a stable POSIX identity.

The product is "Strands Shell"; the simulated Unix user is "lash". These are different things and both are correct.

### Imports

All `use` statements go at the **top of the file** (Rust modules, Python wrapper, JS tests alike). Do not move imports into functions.

### Adding commands

- A coreutils-style command goes in `src/commands/` and is registered with `#[command("name")]`.
- A state-mutating builtin goes in `src/builtins/` and is added to the `lookup()` match in `src/builtins/mod.rs`.
- If a command should appear under WASM too, make sure it's reachable through the WASM lookup path (the macro handles native registration via `inventory` automatically).

### Bindings stay in sync

`src/python.rs` and `src/js.rs` mirror each other. A change to one binding's surface (new method, renamed argument, error mapping) should be reflected in the other unless there's a language-specific reason not to. Node methods are camelCase and return Promises; bytes are `Uint8Array`. Python methods are snake_case; bytes are `bytes`.

### Match surrounding style

Make the smallest reasonable change. Prefer simple, clean solutions over clever ones. Match the formatting of surrounding code. Comments explain *what* the code does or *why* it exists — never temporal context ("recently changed", "used to be"). Run `cargo fmt` and `cargo clippy` on any Rust you touch.

## Security-Sensitive Code

Strands Shell is an **in-process mediation layer**: the `Kernel` boundary is the whole product. Treat changes to the following as security-critical and preserve their guarantees:

- **`src/commands/curl.rs`** and HTTP request handling — `curl`/`http_request` must keep blocking SSRF and metadata-service access (RFC1918, link-local, loopback, IMDS/ECS-task-role) at DNS-resolution time via `SafeResolver`.
- **`src/vfs_kernel.rs`** (incl. `SafeResolver` and bind-path mediation) — file access must stay confined to explicitly bound paths; `readonly` and `direct`/`copy` semantics must hold.
- **Credential handling** — credentials are injected by URL prefix at request time and must not leak across redirects or to non-allowlisted hosts.

A bypass of filesystem mediation, SSRF protection, or credential injection is a **security issue**, not a normal bug. If you find or risk one, follow [SECURITY.md](./SECURITY.md) — do not open a public issue, and never weaken these controls to make a test pass.

## Creating a High-Quality PR

If you are an agent opening a PR on behalf of a contributor, the human is the author and is accountable for everything you submit. A small, focused change that its author fully understands is the single biggest predictor of a fast review and an accepted PR. (See [CONTRIBUTING.md](./CONTRIBUTING.md#using-ai-tools) for the human-facing version.)

- **Understand before you submit.** The contributor must be able to explain why every line works and defend the design. If you produced code you cannot explain plainly, simplify or explain it before opening the PR.
- **Keep it small and focused.** One logical change per PR. A branch that spans the Rust core, the Python binding, and the Node binding is usually several PRs — unless the change is a single cross-cutting surface (e.g. one new method that must exist in both bindings).
- **Open an issue first for anything significant**, so maintainers can align on the approach before time is invested.
- **Don't pad the change.** No drive-by reformatting, unrelated refactors, or speculative abstractions.
- **Run the relevant checks before opening.** Run the test suite(s) for the bindings you touched and make sure the change passes the `ci.yml` merge gate locally. Don't open a PR with known lint, type, or test failures.
- **Actually exercise the change.** Automated checks confirm the code is *valid*, not that the feature *works*. Run the behavior end to end — a script, the CLI, a REPL snippet — and confirm it does what the PR claims, including edge cases.
- **Self-review the diff** end to end as if you were the reviewer, and confirm you can truthfully check every box in the [PR template](./.github/PULL_REQUEST_TEMPLATE.md) — including the item attesting that you have reviewed and understand every line of code in the PR, including any generated by AI tools.

### Commit and PR title conventions

PR titles must follow [Conventional Commits](https://www.conventionalcommits.org/) — this is enforced by `.github/workflows/pr-title.yml`. Allowed types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. Keep the title short; let the body carry the *why*.

## Things to Do

- Keep imports at the top of every file.
- Register coreutils-style commands with `#[command("name")]`; add builtins to `src/builtins/mod.rs`.
- Keep `src/python.rs` and `src/js.rs` in sync when changing the binding surface.
- Run `cargo fmt` and `cargo clippy --workspace --all-targets` on any Rust you touch.
- Run the test suite for the bindings you changed before opening a PR.
- Use Conventional Commit PR titles.

## Things NOT to Do

- **Don't rename the `lash` persona** (`lash`, `/bin/lash`, `USER=lash`, `/home/lash`, `LASH_UID`, `LASH_GID`) — it is the intentional emulated-POSIX identity, not the product name.
- Don't add `fork`/`exec` or direct syscalls — all effects must go through the `Kernel`.
- Don't weaken SSRF guards, bind-path mediation, or credential isolation to make something pass.
- Don't put `use` statements inside functions.
- Don't let the Python and Node bindings drift apart without a stated reason.
- Don't open a PR with a title that fails the conventional-commits check.

## Additional Resources

- [CONTRIBUTING.md](./CONTRIBUTING.md) — human contributor guidelines, full development environment setup
- [SECURITY.md](./SECURITY.md) — vulnerability reporting
- [README.md](./README.md) — product overview, configuration, supported commands
- [COMMANDS.md](./COMMANDS.md) — per-command status and known gaps
- [Strands Agents Documentation](https://strandsagents.com/)
