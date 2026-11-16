# Differential tests

Each case in `cases/` is a small shell script. Its `.expected` file holds what
bash printed for it. `cargo test --test differential` runs every case through
strands-shell and compares the result with the recorded one.

The comparison checks behavior, not exact bytes. Error message text is never
compared, only whether stderr was empty.

## Adding a case

1. Write `cases/<name>.sh`. The script starts in an empty directory, so create
   any files it needs. Avoid output that depends on the machine: no network,
   clock, absolute paths, `$HOME` or user names.
2. Record it from bash (below) and review the new `.expected` file.
3. Run `cargo test --test differential`.

## Header directives

Comment lines at the top of a case can set:

- `# compare: stdout, status` picks the rules to check. Without it the rules
  are `stdout, status, stderr-presence`.
- `# known-difference: <reason>` marks a bug or gap, e.g.
  `# known-difference: fixed by #230`. The case still runs. While it differs
  it's listed in the test output (`cargo test --test differential --
  --nocapture`) and doesn't fail. Once it matches bash the test fails and asks
  you to remove the marker.
- `# skip: <reason>` doesn't run the case. Use it only for differences we keep
  on purpose.

Rules:

| Rule | Passes when |
|---|---|
| `stdout` | stdout is identical |
| `stdout-lines` | stdout has the same lines, in any order |
| `stdout-trimmed` | stdout matches ignoring trailing whitespace on each line and trailing blank lines |
| `status` | exit status is identical |
| `status-class` | both exit 0, or both exit non-zero |
| `stderr-presence` | stderr is empty in both, or non-empty in both |

## Re-recording

```bash
DIFFERENTIAL_RECORD=1 cargo test --test differential
```

This runs every case with `bash` in a fresh directory, with an empty
environment except `PATH=/usr/bin:/bin`, `HOME` set to that directory and
`LC_ALL=C`, and rewrites all `.expected` files. If any case exits 127 (usually
a missing tool) nothing is written. Normal runs don't need bash. The current
files were recorded on Linux with bash 5.2.15, GNU coreutils 8.32, GNU diffutils 3.8 and jq 1.8.1.

Files under `cases/` are stored byte for byte (see `.gitattributes`), so
expected output with `\r\n` stays intact on Windows.
