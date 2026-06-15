# Commands

Strands Shell reimplements a curated subset of POSIX/coreutils in Rust — the
operations agents reach for most, not the full toolset. This is the honest
inventory: status and the notable gaps (missing flags/features and known
divergences from GNU/BSD), validated by running the binary against the system
tools.

## Cross-cutting behavior

These apply across commands, so they're stated once here rather than repeated in
every row. Status labels below are **Full / Partial / Minimal**.

- **Regex** is the Rust `regex` crate — no backreferences or lookaround anywhere
  (so `grep -P` is unsupported, and GNU BRE escapes aren't translated).
- **`jq`** is [`jaq`](https://github.com/01mf02/jaq), a jq subset.
- **Unsupported flags are rejected**, not ignored — so idioms like `cp -p`,
  `set -o pipefail`, or `ln -sf` fail outright rather than degrading.
- **Multiple file arguments** are mishandled by some commands: `cut`/`uniq` read
  only the first, `head`/`tail` hard-error. (`cat`/`sort`/`wc` handle them.)
- **Bad numbers pass silently:** `test`/`[` and arithmetic `$(( ))` treat
  non-numeric/empty operands as `0` and don't reject malformed input.
- **Stdin under `strands-shell -c`** isn't wired to commands (`bad fd 0`) — use
  an in-shell pipe.
- **`Kernel`-mediated security** (filesystem confinement, SSRF and credential
  controls) is validated separately and is **not** a gap.

## Text processing

| Command | Status | Notable gaps |
|---|---|---|
| `grep` | Partial | No `-P`/backreferences/lookaround, no `-f`. `-o` on empty-matching patterns emits blank lines. |
| `sed` | Partial | No branching (`b`/`t`/`:label`) or multiline (`N`/`D`/`P`); no `-f`. `s///N` replaces wrong match; range `c` prints per-line. |
| `tr` | Partial | Missing `[:punct:]`/`[:cntrl:]`/… and `[c*n]` repeats. `-c` two-set translate uses wrong replacement char. |
| `cut` | Partial | No `-b`/`--complement`. **Reads only the first file** of multiple. |
| `sort` | Partial | No `-c`/`-o`/`-V`/`-h` (`-h` wrongly prints help). Keyed-tie order non-deterministic (no whole-line fallback). Loads input into memory. |
| `uniq` | Partial | No `-w`/`-D`. **Reads only the first file** of multiple. |
| `wc` | Partial | No `-m` (char count) or `-L`. Counts correct; column padding differs. |

## File contents

| Command | Status | Notable gaps |
|---|---|---|
| `cat` | Partial | No `-b`/`-s`/`-A`/`-e`/`-t`, no `-` stdin operand. |
| `head` | Partial | No `-c`, negative `-n`, or `head -5` shorthand. **Multiple files hard-error** (no `==>` headers). |
| `tail` | Partial | No `-c`, `-f`/`-F` follow, or shorthand. **Multiple files hard-error.** |
| `tee` | Partial | No `-i`. Common cases (stdout + files, `-a`) work. |

## File management

| Command | Status | Notable gaps |
|---|---|---|
| `cp` | Partial | `-r` works (incl. nesting); no `-p`/`-a`/`-f`/`-i`/`-v`. `cp f f` silently succeeds. |
| `mv` | Partial | Rename/move/cross-mount work; no `-f`/`-i`/`-n`/`-v`. |
| `rm` | Partial | `-r`/`-f` and symlink non-follow correct; no `-d`/`-i`. |
| `mkdir` | Partial | No `-m`. Bad-option path returns exit 0. |
| `rmdir` | Partial | No `-p`. |
| `touch` | Partial | Create/update-time only; no `-t`/`-d`/`-c`/`-r` (can't set a specific time). |
| `ln` | Partial | `-s` only — **hard links refused** (intentional); no `-f` (so `ln -sf` fails). |
| `chmod` | Full | Octal + symbolic modes honored; no `-R`. |

## Path & system

| Command | Status | Notable gaps |
|---|---|---|
| `ls` | Partial | **`-l` omits owner/group/link-count**; `-a` omits `.`/`..`. No `-t`/`-S`/`-d`/`-F`/`-h`. Always single-column. |
| `basename` | Partial | No `-a`/`-s`. `basename /` and `""` diverge. |
| `dirname` | Partial | Single operand only. **Trailing slash mishandled** (`/usr/lib/` → `/usr/lib`). |
| `readlink` | Partial | Plain read only — no `-f`/`-e`/`-m` (canonicalize). |
| `mktemp` | Partial | No `-u`/`-t`. |
| `date` | Partial | **`%s` not expanded**, unknown specifiers pass through literally; no `-d`/`-r`. UTC only. |
| `env` | Partial | Print only — `env VAR=v cmd`, `-i`, `-u` unsupported (the shell-prefix form works). |
| `echo` | Partial | **Expands escapes by default**; `-e`/`-E` printed literally (can't disable). |
| `pwd` | Full | `-L`/`-P` both work. |
| `sleep` | Partial | No unit suffixes (`0.1s`). Respects shell timeout. |
| `true`/`false` | Full | — |

## Networking

| Command | Status | Notable gaps |
|---|---|---|
| `curl` | Partial | GET/POST/JSON/headers/auth/redirects/`-w` work. **No `--max-time`/`--connect-timeout`/`--retry`** (hang risk), no `-I`/`-F`/`-A`/`-G`/cookie-jar. SSRF + credential controls enforced (security feature). |

## JSON

| Command | Status | Notable gaps |
|---|---|---|
| `jq` (jaq) | Partial | Broad filter coverage. **Missing nested key throws** (breaks `.a.b // default`); no `--arg`/`--argjson`/`-S`; `inputs`/`setpath` absent. |

## Search

| Command | Status | Notable gaps |
|---|---|---|
| `find` | Partial | `-name`/`-type`/`-maxdepth`/`-exec`/`-print0` work. No `-delete`/`-size`/`-mtime`/`-prune`/`-regex`. Children sorted (not readdir order). |
| `xargs` | Partial | Space-separated `-n`/`-I`/`-0`/`-d` work; **attached forms (`-n1`, `-I{}`) and `-t`/`-r`/`-L`/`-P` don't**. Always acts as `-r`. |

## Scripting

| Command | Status | Notable gaps |
|---|---|---|
| `lua` | Full | Sandboxed Lua 5.4 with VFS-backed `io`/`os`. **No metatables** (`setmetatable` removed), no `os.date`/`debug`. No wall-clock timeout under bare `-c`. |

## Shell builtins

| Builtin | Status | Notable gaps |
|---|---|---|
| `test` / `[` | Partial | No `[[ ]]`. **Non-numeric/empty operands treated as `0`** (pass instead of erroring). |
| `printf` | Partial | **No floats** (`%f`/`%e`/`%g` print literally); `%d` doesn't parse `0x`/octal. |
| `read` | Partial | No `-a`/`-n`/`-d`; prefix `IFS=` ignored; `-r` is effectively always on. |
| `set` | Partial | **No `-o`** (so `set -o pipefail` fails); `e`/`u`/`x` only. |
| `trap` | Partial | `EXIT` works; **numeric signals (`trap … 0`) ignored**; `-p`/`-l` no-ops. |
| `alias` | Partial | Defined/listed but **never expanded** at execution. |
| `readonly` | Partial | Enforced but violation returns exit 0; no `-p`. |
| `local` | Partial | Works in functions; silently succeeds outside one. |
| `type` | Partial | No `-t`/`-a`. |
| `cd` | Partial | No `CDPATH`; `-P`/`-L` not parsed. |
| `getopts`, `export`, `unset`, `shift`, `umask`, `hash`, `wait`, `:` | Full | Common usage covered. |

Arithmetic `$(( ))` (in the executor, not a builtin): precedence/bitwise/
ternary/hex/octal work, but **malformed input returns a wrong answer with exit 0**
(`$((2 3))` → `2`, `$((1/0))` → `0`), and post-increment `x++`/`x--` returns the
value without updating the variable.

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
