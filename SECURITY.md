# Security Policy

## Supported Versions

Strands Shell is pre-1.0 and under active development. Security fixes are
applied to the latest released version.


## What Is a Security Issue

Strands Shell is an in-process mediation layer for AI agents, not a hardened
sandbox. A bypass of any control that the Kernel boundary is meant to enforce is
treated as a security issue, including:

- Reading or writing files beyond explicitly bound paths (filesystem
  mediation bypass)
- Defeating SSRF and metadata-service protections (e.g. reaching RFC1918,
  link-local, loopback, or IMDS/ECS-task-role addresses through `curl` or
  `http_request`)
- Exfiltrating or misrouting injected HTTP credentials (credential injection
  bypass)
- Escaping Kernel mediation to make direct syscalls, `fork`/`exec`, or
  otherwise reach the host environment
- Crafted input that causes the shell engine to panic, consume unbounded
  memory, or hang indefinitely (e.g. unbounded recursion in the parser, Lua
  memory exhaustion without limits)
- Injected credentials appearing in command output, error messages,
  environment variable dumps, or the Lua scripting context accessible to the
  agent
- Using symlinks, `..` components, or race conditions in bind mounts to
  escape the declared filesystem boundary
- Discrepancies between how the SSRF guard parses a URL and how the HTTP
  client interprets it (e.g. userinfo injection, encoding tricks, scheme
  confusion)
- Any mechanism that causes credentials to be sent to a destination other
  than the originally-matched URL prefix, including via redirects
- One MCP client session accessing state (VFS, environment, credentials)
  belonging to another session on the same server process


## Security Architecture

The security boundary is the **Kernel trait** (`src/os.rs`). All filesystem,
network, and credential operations flow through this interface. The default
`VfsKernel` implementation enforces:

1. **Filesystem isolation** — in-memory VFS with explicit bind mounts; no path
   can escape declared mounts
2. **Network SSRF guard** — two-layer check: URL-level parse + DNS-resolution-time
   IP filtering via `SafeResolver`. Blocks RFC1918, link-local, loopback, IMDS,
   IPv4-mapped-IPv6, 6to4, Teredo
3. **Credential injection** — per-URL prefix matching with path-boundary checks;
   credentials injected only on original request, stripped on redirects
4. **No syscalls** — pure userspace shell; no `fork`, `exec`, or raw syscall paths

Custom Kernel implementations (for embedding in other runtimes) carry their own
security properties. Reports about the `VfsKernel` implementation are in scope;
reports about third-party Kernel implementations should go to those maintainers.


## Out of Scope

The following are explicitly **not** part of the security boundary and will not
be treated as security issues:

- Resource exhaustion via CPU/memory within configured limits (limits are
  best-effort, use OS-level cgroups for hard guarantees)
- Speculative execution and side-channel attacks (same-process architecture,
  use VM isolation for this threat model)
- Multi-tenancy within a single OS process (documented non-goal, one Shell
  instance per session is the contract)
- Agent reading files it was explicitly granted access to via binds (working
  as designed, the bind is the grant)
- Lua scripts consuming memory up to configured limits (limits are advisory,
  Lua sandbox is not a security boundary)

See the [Security Model](README.md#security-model) in the README for the full
threat model and guidance on running Strands Shell inside VM- or container-level
isolation when stronger guarantees are required.


## Reporting Security Issues

Amazon Web Services (AWS) is dedicated to the responsible disclosure of security vulnerabilities.

We kindly ask that you **do not** open a public GitHub issue to report security concerns.

Instead, please submit the issue to the AWS Vulnerability Disclosure Program via [HackerOne](https://hackerone.com/aws_vdp) or send your report via [email](mailto:aws-security@amazon.com).

For more details, visit the [AWS Vulnerability Reporting Page](http://aws.amazon.com/security/vulnerability-reporting/).

Thank you in advance for collaborating with us to help protect our customers.
