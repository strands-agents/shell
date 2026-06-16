# Commands

Strands Shell reimplements a curated subset of POSIX/coreutils in Rust, targeting
the operations most frequently used by agents rather than full toolset.
This document lists the commands supported and notable
gaps - missing flags, unsupported features, and known divergences from GNU/BSD
behavior.

## General Call outs

The following notes apply to all commands:
- **Regex** uses the Rust `regex` crate — backreferences and lookaround are
  unavailable (consequently `grep -P` is unsupported and GNU BRE escapes are
  not translated).
- **`jq`** is backed by [`jaq`](https://github.com/01mf02/jaq), a jq subset.
- **Unsupported flags are rejected**, not ignored — idioms such as
  `cp -p`, `set -o pipefail`, or `ln -sf` produce errors rather than
  succeeding with incorrect behavior.
- **Multiple file arguments** are not uniformly supported:
  - `cut`/`uniq` read only the first file.
  - `head`/`tail` return a hard error.
  - `cat`/`sort`/`wc` handle multiple files correctly.
- **Malformed numeric input passes silently:** `test`/`[` and arithmetic
  `$(( ))` treat non-numeric or empty operands as `0` without producing an error.
- **Stdin under `strands-shell -c`** is not connected to commands (`bad fd 0`)
  — use an in-shell pipe instead.

## Text processing

| Command | Notable gaps |
|---|---|
| `grep` | No `-P`/backreferences/lookaround, no `-f`. `-o` on empty-matching patterns emits blank lines. |
| `sed` | No branching (`b`/`t`/`:label`) or multiline (`N`/`D`/`P`); no `-f`. `s///N` replaces incorrect match; range `c` emits per-line. |
| `tr` | Missing `[:punct:]`/`[:cntrl:]`/… and `[c*n]` repeats. `-c` two-set translate uses incorrect replacement character. |
| `cut` | No `-b`/`--complement`. Reads only the first file when given multiple. |
| `sort` | No `-c`/`-o`/`-V`/`-h` (`-h` incorrectly prints help). Keyed-tie order is non-deterministic (no whole-line fallback). Loads entire input into memory. |
| `uniq` | No `-w`/`-D`. Reads only the first file when given multiple. |
| `wc` | No `-m` (char count) or `-L`. Counts are correct; column padding differs from coreutils. |

## File contents

| Command | Notable gaps |
|---|---|
| `cat` | No `-b`/`-s`/`-A`/`-e`/`-t`; no `-` stdin operand. |
| `head` | No `-c`, negative `-n`, or `head -5` shorthand. Multiple files produce a hard error (no `==>` headers). |
| `tail` | No `-c`, `-f`/`-F` follow, or shorthand. Multiple files produce a hard error. |
| `tee` | No `-i`. Standard usage (stdout + files, `-a`) is supported. |

## File management

| Command | Notable gaps |
|---|---|
| `cp` | `-r` works (including nested directories); no `-p`/`-a`/`-f`/`-i`/`-v`. `cp f f` silently succeeds. |
| `mv` | Rename, move, and cross-mount operations work; no `-f`/`-i`/`-n`/`-v`. |
| `rm` | `-r`/`-f` and symlink non-follow behave correctly; no `-d`/`-i`. |
| `mkdir` | No `-m`. Invalid-option path returns exit 0. |
| `rmdir` | No `-p`. |
| `touch` | File creation and timestamp update only; no `-t`/`-d`/`-c`/`-r` (cannot set a specific time). |
| `ln` | `-s` only — hard links are refused (by design); no `-f` (consequently `ln -sf` fails). |
| `chmod` | Full support. Octal and symbolic modes; no `-R`. |

## Path & system

| Command | Notable gaps |
|---|---|
| `ls` | `-l` omits owner/group/link-count; `-a` omits `.`/`..`. No `-t`/`-S`/`-d`/`-F`/`-h`. Output is always single-column. |
| `basename` | No `-a`/`-s`. `basename /` and `""` diverge from coreutils. |
| `dirname` | Single operand only. Trailing slash is mishandled (`/usr/lib/` → `/usr/lib`). |
| `readlink` | Plain read only — no `-f`/`-e`/`-m` (canonicalization). |
| `mktemp` | No `-u`/`-t`. |
| `date` | `%s` is not expanded; unknown specifiers pass through literally; no `-d`/`-r`. UTC only. |
| `env` | Print-only — `env VAR=v cmd`, `-i`, `-u` are unsupported (use the shell-prefix form instead). |
| `echo` | Expands escapes by default; `-e`/`-E` are printed literally (escape expansion cannot be disabled). |
| `pwd` | Full support. `-L`/`-P` both supported. |
| `sleep` | No unit suffixes (`0.1s`). Respects shell timeout. |
| `true`/`false` | Full support. |

## Networking

| Command | Notable gaps |
|---|---|
| `curl` | GET/POST/JSON/headers/auth/redirects/`-w` supported. No `--max-time`/`--connect-timeout`/`--retry` (risk of indefinite hang); no `-I`/`-F`/`-A`/`-G`/cookie-jar. SSRF and credential controls are enforced (security feature). |

## JSON

| Command | Notable gaps |
|---|---|
| `jq` (jaq) | Broad filter coverage. Missing nested key throws an error (breaks `.a.b // default` pattern); no `--arg`/`--argjson`/`-S`; `inputs`/`setpath` are absent. |

## Search

| Command | Notable gaps |
|---|---|
| `find` | `-name`/`-type`/`-maxdepth`/`-exec`/`-print0` supported. No `-delete`/`-size`/`-mtime`/`-prune`/`-regex`. Children are sorted alphabetically (not readdir order). |
| `xargs` | Space-separated `-n`/`-I`/`-0`/`-d` supported; attached forms (`-n1`, `-I{}`) and `-t`/`-r`/`-L`/`-P` are not. Implicitly behaves as `-r`. |

## Scripting

| Command | Notable gaps |
|---|---|
| `lua` | Full support. Sandboxed Lua 5.4 with VFS-backed `io`/`os`. Metatables are disabled (`setmetatable` removed); no `os.date`/`debug`. No wall-clock timeout under bare `-c`. |

## Shell builtins

| Builtin | Notable gaps |
|---|---|
| `test` / `[` | No `[[ ]]`. Non-numeric/empty operands are treated as `0` (succeeds rather than producing an error). |
| `printf` | No floating-point support (`%f`/`%e`/`%g` print literally); `%d` does not parse `0x`/octal prefixes. |
| `read` | No `-a`/`-n`/`-d`; prefix `IFS=` is ignored; `-r` is effectively always enabled. |
| `set` | No `-o` (consequently `set -o pipefail` fails); only `e`/`u`/`x` are supported. |
| `trap` | `EXIT` works; numeric signals (`trap … 0`) are ignored; `-p`/`-l` are no-ops. |
| `alias` | Can be defined and listed but aliases are never expanded during execution. |
| `readonly` | Enforced, but violations return exit 0; no `-p`. |
| `local` | Works within functions; silently succeeds when used outside a function. |
| `type` | No `-t`/`-a`. |
| `cd` | No `CDPATH`; `-P`/`-L` are not parsed. |
| `getopts`, `export`, `unset`, `shift`, `umask`, `hash`, `wait`, `:` | Full support. |
| `$(( ))` arithmetic | Precedence, bitwise, ternary, hex, and octal work correctly. Malformed input returns an incorrect result with exit 0 (`$((2 3))` → `2`, `$((1/0))` → `0`). Post-increment `x++`/`x--` returns the value without updating the variable. |

## Shell language

| Feature | Supported |
|---|---|
| Pipelines & lists | `\|`, `&&`, `\|\|`, `;` |
| Redirections | `>`, `>>`, `<`, `2>`, `&>`, here-docs `<<` |
| Conditionals | `if`/`elif`/`else`, `case` |
| Loops | `for`, `while`, `until` |
| Functions | definitions, `local` variables, `return` |
| Grouping | command groups `{ }`, subshells `( )` |
| Expansion | variables (`${VAR:-default}`, `${VAR%pat}`, `${#VAR}`), command subst (`` `cmd` ``, `$(cmd)`), arithmetic `$(( ))`, globs |
| Quoting | single, double |
| Jobs | background `&` |
| Scripts | `. script.sh` / `source` |
