//! Repo automation, run via `cargo xtask <task>`.
//!
//! The headline task is `check`, which runs the same gate as CI
//! (`.github/workflows/ci.yml`) so a green `cargo xtask check` locally means a
//! green PR. Rust checks always run; the Python and Node binding checks run
//! only when their toolchain and local setup are present (so the command is
//! useful whether or not you've built the bindings), and are skipped with a
//! note otherwise.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};

fn main() {
    let task = env::args().nth(1);
    match task.as_deref() {
        Some("check") => check(parse_check_args()),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
        }
        Some(other) => {
            eprintln!("xtask: unknown task `{other}`\n");
            print_help();
            exit(2);
        }
    }
}

fn print_help() {
    eprintln!(
        "\
cargo xtask — repo automation

USAGE:
    cargo xtask <task> [options]

TASKS:
    check        Run the full CI gate locally (fmt, clippy, test, doc + bindings)
    help         Show this message

CHECK OPTIONS:
    --rust-only  Skip the Python and Node binding checks
    --no-clippy  Skip the advisory clippy run
"
    );
}

struct CheckArgs {
    rust_only: bool,
    clippy: bool,
}

fn parse_check_args() -> CheckArgs {
    let mut args = CheckArgs {
        rust_only: false,
        clippy: true,
    };
    for arg in env::args().skip(2) {
        match arg.as_str() {
            "--rust-only" => args.rust_only = true,
            "--no-clippy" => args.clippy = false,
            other => {
                eprintln!("xtask check: unknown option `{other}`");
                exit(2);
            }
        }
    }
    args
}

fn check(args: CheckArgs) {
    let root = repo_root();
    let mut steps: Vec<Step> = Vec::new();

    // ---- Rust core (always; mirrors the `rust` CI job) ----
    steps.push(Step::always(
        "cargo fmt --check",
        cargo(&root, &["fmt", "--all", "--", "--check"]),
    ));
    if args.clippy {
        // Advisory, not `-D warnings`: clippy is not a CI merge gate yet (the
        // tree still carries warnings), so a lint shouldn't fail `check`. It
        // still prints findings for anything you touched.
        steps.push(Step::advisory(
            "cargo clippy (advisory)",
            cargo(&root, &["clippy", "--workspace", "--all-targets"]),
        ));
    }
    steps.push(Step::always(
        "cargo test",
        cargo(&root, &["test", "--workspace", "--all-targets"]),
    ));
    steps.push(Step::always("cargo doc", {
        // CI gates docs with `-D warnings` via RUSTDOCFLAGS.
        let mut c = cargo(&root, &["doc", "--workspace", "--no-deps"]);
        c.env("RUSTDOCFLAGS", "-D warnings");
        c
    }));

    // ---- Bindings (conditional; mirror the `python` and `node` CI jobs) ----
    if !args.rust_only {
        match python_runner(&root) {
            Some((py, label)) => {
                // Rebuild the extension into the venv, then run pytest, so the
                // tests exercise the current code rather than a stale wheel.
                let mut develop = Command::new(&py);
                develop
                    .current_dir(&root)
                    .args(["-m", "maturin", "develop", "--release"]);
                steps.push(Step::always("maturin develop", develop));

                let mut pytest = Command::new(&py);
                pytest
                    .current_dir(&root)
                    .args(["-m", "pytest", "tests/python", "-q"]);
                steps.push(Step::always(&format!("pytest ({label})"), pytest));
            }
            None => steps.push(Step::skipped(
                "python",
                "no .venv with maturin+pytest (run: python -m venv .venv && \
                 .venv/bin/pip install maturin pytest)",
            )),
        }

        if root.join("node_modules").is_dir() && have("npm") {
            let mut build = Command::new("npm");
            build.current_dir(&root).args(["run", "build:debug"]);
            steps.push(Step::always("npm run build:debug", build));

            let mut typecheck = Command::new("npm");
            typecheck.current_dir(&root).args(["run", "typecheck"]);
            steps.push(Step::always("tsc typecheck", typecheck));

            let mut test = Command::new("npm");
            test.current_dir(&root).arg("test");
            steps.push(Step::always("npm test", test));
        } else {
            steps.push(Step::skipped("node", "no node_modules (run: npm install)"));
        }
    }

    run_steps(steps);
}

/// One pipeline step: either a command to run, or a skip notice.
struct Step {
    label: String,
    command: Option<Command>,
    skip_reason: Option<String>,
    /// Advisory steps run and report, but their failure does not fail `check`.
    advisory: bool,
}

impl Step {
    fn always(label: &str, command: Command) -> Self {
        Step {
            label: label.to_string(),
            command: Some(command),
            skip_reason: None,
            advisory: false,
        }
    }

    fn advisory(label: &str, command: Command) -> Self {
        Step {
            label: label.to_string(),
            command: Some(command),
            skip_reason: None,
            advisory: true,
        }
    }

    fn skipped(label: &str, reason: &str) -> Self {
        Step {
            label: label.to_string(),
            command: None,
            skip_reason: Some(reason.to_string()),
            advisory: false,
        }
    }
}

fn run_steps(steps: Vec<Step>) {
    let mut failures: Vec<String> = Vec::new();
    let mut skips: Vec<String> = Vec::new();

    for step in steps {
        match (step.command, step.skip_reason) {
            (Some(mut command), _) => {
                eprintln!("\n\x1b[1m▸ {}\x1b[0m", step.label);
                let status = command.status();
                let ok = matches!(&status, Ok(s) if s.success());
                if ok {
                    continue;
                }
                let detail = match status {
                    Ok(s) => format!(
                        "exit {}",
                        s.code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "signal".into())
                    ),
                    Err(e) => format!("failed to launch: {e}"),
                };
                if step.advisory {
                    // Report but don't fail the run.
                    eprintln!(
                        "\x1b[33m! {} ({detail}) — advisory, not fatal\x1b[0m",
                        step.label
                    );
                } else {
                    eprintln!("\x1b[31m✗ {} ({detail})\x1b[0m", step.label);
                    failures.push(step.label);
                }
            }
            (None, Some(reason)) => {
                eprintln!("\n\x1b[33m∅ {} skipped — {reason}\x1b[0m", step.label);
                skips.push(step.label);
            }
            (None, None) => {}
        }
    }

    eprintln!();
    if !skips.is_empty() {
        eprintln!("\x1b[33mskipped: {}\x1b[0m", skips.join(", "));
    }
    if failures.is_empty() {
        eprintln!("\x1b[32m✓ all checks passed\x1b[0m");
    } else {
        eprintln!("\x1b[31m✗ failed: {}\x1b[0m", failures.join(", "));
        exit(1);
    }
}

/// A `cargo` invocation rooted at the repo.
fn cargo(root: &Path, args: &[&str]) -> Command {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut c = Command::new(cargo);
    c.current_dir(root).args(args);
    c
}

/// Find a Python interpreter in a local `.venv` that has both maturin and
/// pytest installed. Returns the interpreter path and a short label.
fn python_runner(root: &Path) -> Option<(PathBuf, String)> {
    // venv layout differs by platform: bin/ on Unix, Scripts/ on Windows.
    let candidates = [
        root.join(".venv/bin/python"),
        root.join(".venv/Scripts/python.exe"),
    ];
    let py = candidates.into_iter().find(|p| p.is_file())?;
    // Confirm the tools we need are importable before committing to the step.
    let ok = Command::new(&py)
        .args(["-c", "import maturin, pytest"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok.then(|| (py, ".venv".to_string()))
}

/// Whether an executable is on PATH.
fn have(bin: &str) -> bool {
    // `<bin> --version` is a cheap, side-effect-free presence probe.
    Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The workspace root (the xtask crate lives one level below it).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live in a subdirectory of the repo root")
        .to_path_buf()
}
