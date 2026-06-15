# Contributing Guidelines

Thank you for your interest in contributing to our project. Whether it's a bug report, new feature, correction, or additional
documentation, we greatly value feedback and contributions from our community.

Please read through this document before submitting any issues or pull requests to ensure we have all the necessary
information to effectively respond to your bug report or contribution.


## Reporting Bugs/Feature Requests

We welcome you to use the [Bug Reports](../../issues/new?template=bug_report.yml) form to report bugs or [Feature Requests](../../issues/new?template=feature_request.yml) to suggest features.

For a list of known bugs and feature requests:
- Check [Bug Reports](../../issues?q=is%3Aissue%20state%3Aopen%20label%3Abug) for currently tracked issues
- See [Feature Requests](../../issues?q=is%3Aissue%20state%3Aopen%20label%3Aenhancement) for requested enhancements

When filing an issue, please check for already tracked items.

Please try to include as much information as you can. Details like these are incredibly useful:

* A reproducible test case or series of steps
* The binding you are using (Python, Node.js, Rust, or WASM) and its version
* The version of our code being used (commit ID)
* Any modifications you've made relevant to the bug
* Anything unusual about your environment or deployment


## Finding contributions to work on
Looking at the existing issues is a great way to find something to contribute to. We label issues that are well-defined and ready for community contributions with the "ready for contribution" label.

Check our [Ready for Contribution](../../issues?q=is%3Aissue%20state%3Aopen%20label%3A%22ready%20for%20contribution%22) issues for items you can work on.

Before starting work on any issue:
1. Check if someone is already assigned or working on it
2. Comment on the issue to express your interest and ask any clarifying questions
3. Wait for maintainer confirmation before beginning significant work


## Development Tenets
Our team follows these core principles when designing and implementing features. These tenets help us make consistent decisions, resolve trade-offs, and maintain the quality and coherence of Strands Shell. When contributing, please consider how your changes align with these principles:

1. **Simple at any scale:** We believe that simple things should be simple. The same clean abstractions that power a weekend prototype should scale effortlessly to production workloads. We reject the notion that enterprise-grade means enterprise-complicated - Strands remains approachable whether it's your first agent or your millionth.
2. **Extensible by design:** We allow for as much configuration as possible, from the `Kernel` trait to filesystem binds, credential injection, and MCP integration. We meet customers where they are with flexible extension points that are simple to integrate with.
3. **Composability:** Primitives are building blocks with each other. Each feature of Strands Shell is developed with all other features in mind, they are consistent and complement one another.
4. **The obvious path is the happy path:** Through intuitive naming, helpful error messages, and thoughtful API design, we guide developers toward correct patterns and away from common pitfalls.
5. **We are accessible to humans and agents:** Strands Shell is designed for both humans and AI to understand equally well. We don't take shortcuts on curated DX for humans and we go the extra mile to make sure coding assistants can help you use those interfaces the right way.
6. **Embrace common standards:** We respect what came before, and do not want to reinvent something that is already widely adopted or done better. Strands Shell is Bourne-compatible and speaks established protocols like MCP.

When proposing solutions or reviewing code, we reference these principles to guide our decisions. If two approaches seem equally valid, we choose the one that best aligns with our tenets.

## Development Environment

Strands Shell is a Rust core that compiles to several targets: a native binary, a Python extension module (via [PyO3](https://pyo3.rs/)/[maturin](https://www.maturin.rs/)), Node.js bindings (via [napi-rs](https://napi.rs/)), and a `wasm32-wasip2` WebAssembly module. All bindings build from the same crate, so the Rust toolchain is required for every workflow.

| Area | Tooling | Builds from |
|------|---------|-------------|
| Shell core / Rust crate | `cargo` | `Cargo.toml` |
| Python bindings | `maturin` + `pytest` | `pyproject.toml` (`python` feature) |
| Node.js bindings | `npm` + `@napi-rs/cli` | `package.json` (`node` feature) |
| WASM module | `scripts/build-wasm.sh` | `wasm` feature |

### Prerequisites

- **Rust** 1.85+ (stable; required by Rust edition 2024). Install via [rustup](https://rustup.rs/). `cargo fmt` and `cargo clippy` require the `rustfmt` and `clippy` components (included with the default profile).
- **Python** 3.10+ (only for the Python bindings).
- **Node.js** 18+ (only for the Node.js bindings).
- **wasi-sdk** 32+ (only for the WASM target).

### One command to check everything

To run the full CI gate locally before opening a PR — formatting, clippy, the
Rust test suite, docs, and (when their toolchains are set up) the Python and
Node binding tests — use:

```bash
cargo xtask check              # everything; mirrors .github/workflows/ci.yml
cargo xtask check --rust-only  # skip the Python/Node binding checks
```

A green `cargo xtask check` locally means a green PR. The Python/Node steps are
skipped with a note if their setup isn't present, so the command works even if
you only build the Rust core. The individual commands are below if you'd rather
run them piecemeal.

### Rust (shell core)

The crate is the source of truth for all bindings. From the repository root:

```bash
cargo build                        # build the library and binaries
cargo test --workspace --all-targets  # run unit and integration tests
cargo fmt                          # format
cargo clippy --workspace --all-targets # lint
cargo doc --workspace --no-deps --open # build and view the API reference
```

Integration tests live in `tests/` (`shell_integration.rs`, `curl_integration.rs`, `lua_integration.rs`, `mcp_integration.rs`, `vfs_unit.rs`).

### Python bindings

The Python bindings are built with `maturin`, which compiles the crate with the `python` feature and installs it into your active environment.

```bash
# Create and activate a virtual environment (recommended)
python -m venv .venv
source .venv/bin/activate  # On Windows: .venv\Scripts\activate

# Install build and test dependencies
pip install maturin pytest

# Build the extension module and install it into the virtualenv in editable mode
maturin develop --features python

# Run the Python test suite
pytest tests/python -v
```

The Python sources live under `python/strands_shell/`; the compiled module is `strands_shell._native`.

### Node.js bindings

The Node.js bindings use `@napi-rs/cli` to compile the crate with the `node` feature into a native addon.

```bash
npm install            # install dependencies
npm run build          # release build of the native addon
npm run build:debug    # faster debug build for local development
npm test               # run the Node.js test suite (tests/js/*.mjs)
npm run typecheck      # tsc --noEmit over the public .d.ts (tests/ts/)
```

The shipped TypeScript declarations (`index.d.ts`, `native.d.ts`) are
hand-authored; `npm run typecheck` type-checks them against a usage test in
`tests/ts/` so they can't silently drift from the JS implementation.

### WASM module

The WASM target compiles to `wasm32-wasip2` and needs wasi-sdk 32 or newer:

```bash
./scripts/build-wasm.sh --release

# Run a command through the built module with wasmtime
echo 'echo hello from wasm' | wasmtime -W exceptions=y -S http strands-shell-wasm.wasm
```

The WASM build is a reduced surface: it reads commands from stdin and writes to
stdout/stderr, with no PyO3/Node bindings, no built-in MCP server, and no
`--config` file. `curl` requires the WASI host to grant outbound HTTP (the
`-S http` flag above). It is a build target, not a published release artifact.

### Code Formatting and Style Guidelines

If you touched Rust, please run formatting and lint before submitting a pull
request (these aren't enforced in CI yet, but keeping diffs clean helps
reviewers):

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets

# Python
pytest tests/python

# Node.js
npm test
```

If you're using an IDE, consider configuring it to run `rustfmt` and `clippy` automatically.

## Using AI Tools

We love AI. We build with coding agents every day, and you're welcome to use them too — they're a great way to move fast and explore a codebase.

That said, **you are the author of your pull request, not your agent.** Before you open a PR, make sure you understand the code well enough to explain why it works, defend the design choices, and maintain it if asked. If you couldn't walk a reviewer through it line by line, it's not ready yet.

A few things that help us help you:

- **Keep changes small and incremental.** A focused PR that does one thing is far easier for us to understand, guide, and merge than a large one that touches many areas. When in doubt, split it up.
- **Open an issue first for anything significant**, so we can align on the approach before you (or your agent) invest the time.
- **Review every line your agent generates.** Delete what you don't need, simplify what's over-engineered, and make sure tests actually exercise the behavior — not just pass.

High-quality PRs get reviewed faster and are far more likely to be accepted. Taking the time to understand and trim your changes is the single best thing you can do to get them merged.

## Contributing via Pull Requests
Contributions via pull requests are much appreciated. Before sending us a pull request, please ensure that:

1. You are working against the latest source on the default branch.
2. You check existing open, and recently merged, pull requests to make sure someone else hasn't addressed the problem already.
3. You open an issue to discuss any significant work - we would hate for your time to be wasted.

To send us a pull request, please:

1. Create a branch.
2. Modify the source; please focus on the specific change you are contributing. If you also reformat all the code, it will be hard for us to focus on your change.
3. Format your code with `cargo fmt` (and run `cargo clippy`).
4. Ensure local tests pass for the bindings you touched: `cargo test --workspace --all-targets`, `pytest tests/python`, and/or `npm test`.
5. Commit to your branch using clear commit messages following the [Conventional Commits](https://www.conventionalcommits.org) specification.
6. Send us a pull request, answering any default questions in the pull request interface.
7. Pay attention to any automated CI failures reported in the pull request, and stay involved in the conversation.


## Code of Conduct
This project has adopted the [Amazon Open Source Code of Conduct](https://aws.github.io/code-of-conduct).
For more information see the [Code of Conduct FAQ](https://aws.github.io/code-of-conduct-faq) or contact
opensource-codeofconduct@amazon.com with any additional questions or comments.


## Security issue notifications
If you discover a potential security issue in this project we ask that you notify AWS/Amazon Security via our [vulnerability reporting page](http://aws.amazon.com/security/vulnerability-reporting/). Please do **not** create a public github issue. Bypasses of filesystem mediation, SSRF protection, or credential injection are treated as security issues.


## Licensing

See the [LICENSE](./LICENSE) file for our project's licensing. We will ask you to confirm the licensing of your contribution.
