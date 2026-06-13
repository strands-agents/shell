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

Some behaviors are explicitly out of scope. Best-effort resource limits
(timeouts, output caps, fd/inode limits, pipeline depth), speculative side
channels (Spectre and similar), and multi-tenancy within a single process are
**not** part of the security boundary. See the
[Security Model](README.md#security-model) in the README for the full threat
model and guidance on running Strands Shell inside VM- or container-level
isolation when stronger guarantees are required.

## Reporting Security Issues

Amazon Web Services (AWS) is dedicated to the responsible disclosure of security vulnerabilities.  
  
We kindly ask that you **do not** open a public GitHub issue to report security concerns.  
  
Instead, please submit the issue to the AWS Vulnerability Disclosure Program via [HackerOne](https://hackerone.com/aws_vdp) or send your report via [email](mailto:aws-security@amazon.com).  
  
For more details, visit the [AWS Vulnerability Reporting Page](http://aws.amazon.com/security/vulnerability-reporting/).  

Thank you in advance for collaborating with us to help protect our customers.
