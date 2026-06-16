#!/usr/bin/env bash
#
# Demonstrate Cedar authorization policies in strands-shell.
#
# For each example policy under examples/policies/, this runs a couple of
# commands through `strands-shell --policy <file>` and shows which are allowed
# and which are denied. Denials print `policy denied: <action> ...` to stderr
# and exit non-zero; the built-in SSRF and filesystem protections still apply
# underneath, so a policy can only ever *add* restrictions.
#
# Usage:
#   ./examples/run-policies.sh           # builds the debug binary, then runs
#   SHELL_BIN=/path/to/strands-shell ./examples/run-policies.sh   # use a prebuilt binary
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POLICIES="$ROOT/examples/policies"

# Locate the binary: honor SHELL_BIN, otherwise build the debug binary.
if [[ -n "${SHELL_BIN:-}" ]]; then
  BIN="$SHELL_BIN"
else
  echo "Building strands-shell (debug)..." >&2
  cargo build -q --bin strands-shell --manifest-path "$ROOT/Cargo.toml"
  BIN="$ROOT/target/debug/strands-shell"
fi

# Run one command under a policy and report allowed/denied based on exit code.
# Usage: demo <policy-file> <expectation> <command>
#   <expectation> is "allow" or "deny" — printed so output is self-checking.
demo() {
  local policy="$1" expect="$2" cmd="$3"
  printf '  $ strands-shell --policy %s -c %q\n' "$(basename "$policy")" "$cmd"
  local out status
  out="$("$BIN" --policy "$policy" -c "$cmd" 2>&1)" && status=0 || status=$?
  if [[ -n "$out" ]]; then
    sed 's/^/    /' <<<"$out"
  fi
  if [[ $status -eq 0 ]]; then
    printf '    => exit 0 (allowed)        [expected: %s]\n\n' "$expect"
  else
    printf '    => exit %d (denied)        [expected: %s]\n\n' "$status" "$expect"
  fi
}

echo
echo "=== Example 1: read-only sandbox ==========================================="
echo "Reads are allowed; any mutation, network, or env access is denied."
echo
demo "$POLICIES/01-read-only.cedar" allow 'ls / | sort | head -n 5'
demo "$POLICIES/01-read-only.cedar" deny  'echo hi > /home/lash/x.txt'
demo "$POLICIES/01-read-only.cedar" deny  'curl https://example.com/'

echo "=== Example 2: workspace jail =============================================="
echo "Full read/write inside /home/lash/workspace; everything else is denied."
echo
demo "$POLICIES/02-workspace-jail.cedar" allow \
  'mkdir /home/lash/workspace; echo hi > /home/lash/workspace/f.txt; cat /home/lash/workspace/f.txt'
demo "$POLICIES/02-workspace-jail.cedar" deny \
  'echo hi > /home/lash/escape.txt'

echo "=== Example 3: mixed controls with a forbid override ======================="
echo "Read anywhere, write under home, never read *.secret, GET example.com only."
echo
demo "$POLICIES/03-mixed-controls.cedar" allow \
  'echo note > /home/lash/n.txt; cat /home/lash/n.txt'
demo "$POLICIES/03-mixed-controls.cedar" deny \
  'echo s > /home/lash/k.secret; cat /home/lash/k.secret'
demo "$POLICIES/03-mixed-controls.cedar" deny \
  'env'

echo "Done. Denied commands printed 'policy denied: <action> ...' and exited non-zero."
