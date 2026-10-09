# Examples

## Cedar authorization policies

[`policies/`](policies/) contains three Cedar policies of increasing
complexity, and [`run-policies.sh`](run-policies.sh) demonstrates each one by
running a few commands and showing which are allowed and which are denied.

```sh
./examples/run-policies.sh
```

The script builds the debug `strands-shell` binary and runs the examples. To
use a prebuilt binary instead:

```sh
SHELL_BIN=/path/to/strands-shell ./examples/run-policies.sh
```

| Policy | What it shows |
|---|---|
| [`01-read-only.cedar`](policies/01-read-only.cedar) | A single `permit` for read-only actions; everything else is denied by default. |
| [`02-workspace-jail.cedar`](policies/02-workspace-jail.cedar) | Matching `context.input.path` with `like`, scoped to the actions that carry a path. |
| [`03-mixed-controls.cedar`](policies/03-mixed-controls.cedar) | Layered permits, a `forbid` override for `*.secret`, and a scoped network rule. |
| [`04-egress-allowlist.cedar`](policies/04-egress-allowlist.cedar) | Anti-exfiltration: network only as `GET` to one host — no other hosts, no `POST`. |
| [`05-shield-secrets.cedar`](policies/05-shield-secrets.cedar) | Read/write a project tree but `forbid` reading `.env` / `.pem` / `.ssh` / credentials. |

Examples 4 and 5 target the ["lethal trifecta"](https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/)
— private data + untrusted input + external communication. Each cuts one leg:
04 removes the exfiltration channel, 05 removes the agent's access to secrets.

See the [Authorization Policies](../README.md#authorization-policies-cedar)
section of the main README for the action vocabulary and how policies compose
with the rest of the sandbox.
