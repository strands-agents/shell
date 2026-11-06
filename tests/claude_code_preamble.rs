//! Claude Code's Bash tool wraps every command in a bash preamble. Each construct in it must run
//! through the shell without a diagnostic, as it does through bash.
//!
//! `fixtures/claude-code-snapshot-script.sh` is the snapshot-creation script Claude Code 2.1.285
//! passed to `bash -c -l`, with only the home path generalised. [`wrapper`] builds the per-command
//! string it passed to `bash -c`, less its `{ \builtin unalias …; } >/dev/null 2>&1` clause.

use strands_shell::Shell;

const SNAPSHOT_FILE: &str =
    "/home/lash/.claude/shell-snapshots/snapshot-bash-1790732593649-42pi4m.sh";
const CWD_FILE: &str = "/home/lash/claude-1234-cwd";

fn snapshot_script() -> String {
    include_str!("fixtures/claude-code-snapshot-script.sh").replace("\r\n", "\n")
}

/// The per-command wrapper without its group clause, with one command inserted.
fn wrapper(snapshot: Option<&str>, command: &str) -> String {
    let mut clauses = Vec::new();
    if let Some(snapshot) = snapshot {
        clauses.push(format!("source {snapshot} 2>/dev/null || true"));
    }
    clauses.push("shopt -u extglob 2>/dev/null || true".to_string());
    clauses.push(format!("eval '{command}' < /dev/null"));
    clauses.push(format!("pwd -P >| {CWD_FILE}"));
    clauses.join(" && ")
}

fn rt() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    (rt, tokio::task::LocalSet::new())
}

async fn run(command: &str) -> strands_shell::Output {
    let mut shell = Shell::builder().build().unwrap();
    shell.run(command).await
}

/// A shell whose home holds an empty `.bashrc`, so the snapshot script records functions.
async fn shell_with_bashrc() -> Shell {
    let mut shell = Shell::builder().build().unwrap();
    let output = shell
        .run("mkdir -p /home/lash/.claude/shell-snapshots && : > /home/lash/.bashrc")
        .await;
    assert_eq!(output.status, 0, "{}", output.stderr);
    shell
}

#[test]
fn the_snapshot_script_writes_its_file_and_the_wrapper_is_silent() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let mut shell = shell_with_bashrc().await;
        let created = shell.run(&snapshot_script()).await;
        assert_eq!(created.status, 0, "stderr: {}", created.stderr);
        let written = shell.run(&format!("cat {SNAPSHOT_FILE}")).await;
        for section in [
            "# Aliases",
            "function rg {",
            "function pkill {",
            "export PATH=",
        ] {
            assert!(
                written.stdout.contains(section),
                "missing {section:?} in\n{}",
                written.stdout
            );
        }

        let ran = shell
            .run(&wrapper(Some(SNAPSHOT_FILE), "printf %s done"))
            .await;
        assert_eq!(ran.stderr, "", "the wrapper must produce no diagnostic");
        assert_eq!(ran.stdout, "done");
        assert_eq!(ran.status, 0);
        let cwd = shell.run(&format!("cat {CWD_FILE}")).await;
        assert_eq!(cwd.stdout, "/home/lash\n");

        let sourced = shell
            .run(&wrapper(Some(SNAPSHOT_FILE), "type pkill; type grep"))
            .await;
        assert_eq!(sourced.stderr, "");
        assert_eq!(
            sourced.stdout, "pkill is a shell function\ngrep is a shell function\n",
            "the snapshot was sourced, not skipped behind its 2>/dev/null"
        );
    }));
}

#[test]
fn shopt_records_state_and_refuses_an_unknown_name() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out = run("shopt -q extglob; echo a=$?; shopt -s extglob; shopt -q extglob; echo b=$?; shopt -p extglob; shopt -u extglob; shopt -p extglob; shopt nosuch; echo c=$?").await;
        assert_eq!(
            out.stdout,
            "a=1\nb=0\nshopt -s extglob\nshopt -u extglob\nc=1\n"
        );
        assert_eq!(
            out.stderr,
            "strands-shell: shopt: nosuch: invalid shell option name\n"
        );
    }));
}

#[test]
fn the_function_keyword_defines_a_function() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out =
            run("function h { printf '%s-' \"$1\"; }; function k() { printf k; }; h 1; k").await;
        assert_eq!(out.stderr, "");
        assert_eq!(out.stdout, "1-k");
    }));
}

#[test]
fn a_scalar_append_appends() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out = run("x=a; x+=b; unset y; y+=c; printf '%s %s' \"$x\" \"$y\"").await;
        assert_eq!(out.stderr, "");
        assert_eq!(out.stdout, "ab c");
    }));
}

#[test]
fn an_array_defines_inside_a_function_and_is_refused_when_run() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out = run("function pk { local -a probe=(); probe+=(\"$1\"); echo unreached; }; echo defined; pk x; echo rc=$?").await;
        assert_eq!(out.stdout, "defined\nrc=2\n");
        assert_eq!(out.stderr, "strands-shell: arrays are not supported\n");
    }));
}

#[test]
fn a_quoted_word_inside_a_brace_expansion_expands_and_keeps_its_fields() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out = run("set -- 'a b' c; printf '[%s]' ${1+\"$@\"}; printf '|'; printf '[%s]' \"${1+\"$@\"}\"; printf '|'; printf '[%s]' ${3+\"$@\"}; printf '|'; q=inner; echo ${p:-\"${q}\"}; x=val; echo \"${x:+\"[${x}]\"}\"").await;
        assert_eq!(out.stderr, "");
        assert_eq!(out.stdout, "[a b][c]|[a b][c]|[]|inner\n[val]\n");
    }));
}

#[test]
fn a_subscript_on_a_scalar_reads_as_bash_does() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out = run("x=a; echo \"${x[0]}-${x[@]}-${x[*]}-${x[1]}-${#x[@]}-${#y[@]}-${y[@]+set}-${x[@]+set}\"").await;
        assert_eq!(out.stderr, "");
        assert_eq!(out.stdout, "a-a-a--1-0--set\n");
    }));
}

#[test]
fn a_pipeline_can_feed_a_compound_command() {
    let (rt, local) = rt();
    rt.block_on(local.run_until(async {
        let out = run("printf '1\\n2\\n' | while read l; do printf '[%s]' \"$l\"; done; printf '|'; printf 'x' | if read l; then printf \"got $l\"; fi | tr x y; printf '|%s' \"$(printf 'a\\nb\\n' | while read l; do printf '<%s>' \"$l\"; done)\"").await;
        assert_eq!(out.stderr, "");
        assert_eq!(out.stdout, "[1][2]|got y|<a><b>");
    }));
}
