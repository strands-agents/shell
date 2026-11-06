//! Differential tests: each case in `tests/differential/cases/` is a small
//! shell script whose `.expected` file was recorded from bash. This test runs
//! every case through strands-shell and compares the result using the rules
//! named in the case header. See `tests/differential/README.md`.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use strands_shell::{Output, Shell};

const DEFAULT_RULES: &[&str] = &["stdout", "status", "stderr-presence"];
const CASE_TIMEOUT: Duration = Duration::from_secs(30);

struct Case {
    name: String,
    path: PathBuf,
    script: String,
    rules: Vec<String>,
    known_difference: Option<String>,
}

struct Expected {
    status: i32,
    stderr_empty: bool,
    stdout: String,
}

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential/cases")
}

/// Reads a file with line endings normalized to `\n`, so a CRLF checkout on
/// Windows compares the same as on Unix.
fn read_lf(path: &Path) -> String {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.replace("\r\n", "\n")
}

fn load_cases() -> Vec<Case> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(cases_dir())
        .expect("read cases dir")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sh"))
        .collect();
    paths.sort();
    paths.into_iter().map(|path| parse_case(&path)).collect()
}

fn parse_case(path: &Path) -> Case {
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    let script = read_lf(path);
    let mut rules: Vec<String> = DEFAULT_RULES.iter().map(|r| r.to_string()).collect();
    let mut known_difference = None;
    for line in script.lines().take_while(|line| line.starts_with('#')) {
        let line = line.trim_start_matches('#').trim();
        if let Some(list) = line.strip_prefix("compare:") {
            rules = list.split(',').map(|r| r.trim().to_string()).collect();
        } else if let Some(reason) = line.strip_prefix("known-difference:") {
            known_difference = Some(reason.trim().to_string());
        }
    }
    for rule in &rules {
        assert!(
            matches!(
                rule.as_str(),
                "stdout"
                    | "stdout-lines"
                    | "stdout-trimmed"
                    | "status"
                    | "status-class"
                    | "stderr-presence"
            ),
            "{name}: unknown compare rule {rule:?}"
        );
    }
    Case {
        name,
        path: path.to_path_buf(),
        script,
        rules,
        known_difference,
    }
}

fn expected_path(case: &Case) -> PathBuf {
    case.path.with_extension("expected")
}

/// The `.expected` format is a status line, a stderr line, then stdout verbatim:
///
/// ```text
/// status: 0
/// stderr: empty
/// --- stdout
/// hello
/// ```
fn format_expected(status: i32, stderr_empty: bool, stdout: &str) -> String {
    let stderr = if stderr_empty { "empty" } else { "non-empty" };
    format!("status: {status}\nstderr: {stderr}\n--- stdout\n{stdout}")
}

fn parse_expected(case: &Case) -> Expected {
    let path = expected_path(case);
    let text = read_lf(&path);
    let bad = format!("{}: malformed expected file", path.display());
    let (status_line, rest) = text.split_once('\n').expect(&bad);
    let (stderr_line, rest) = rest.split_once('\n').expect(&bad);
    let stdout = rest.strip_prefix("--- stdout\n").expect(&bad);
    Expected {
        status: status_line
            .strip_prefix("status: ")
            .and_then(|s| s.parse().ok())
            .expect(&bad),
        stderr_empty: match stderr_line {
            "stderr: empty" => true,
            "stderr: non-empty" => false,
            _ => panic!("{bad}"),
        },
        stdout: stdout.to_string(),
    }
}

/// Runs the case under bash in a fresh directory and rewrites its `.expected`.
fn record(case: &Case) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("differential")
        .join(&case.name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::process::Command::new("bash")
        .arg(&case.path)
        .current_dir(&dir)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run bash");
    let status = out.status.code().expect("bash killed by signal");
    let stdout = String::from_utf8(out.stdout).expect("bash stdout is UTF-8");
    let text = format_expected(status, out.stderr.is_empty(), &stdout);
    std::fs::write(expected_path(case), text).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Runs the case in a fresh shell on its own thread, so a hang fails the case
/// instead of blocking the whole test.
fn run_strands(case: &Case) -> Option<Output> {
    let script = case.script.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let out = rt.block_on(local.run_until(async {
            let mut shell = Shell::builder().build().unwrap();
            shell.run(&script).await
        }));
        let _ = tx.send(out);
    });
    rx.recv_timeout(CASE_TIMEOUT).ok()
}

fn trimmed(stdout: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn sorted_lines(stdout: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort();
    lines
}

/// Returns the rules the actual output breaks.
fn check(rules: &[String], expected: &Expected, actual: &Output) -> Vec<String> {
    rules
        .iter()
        .filter(|rule| {
            let ok = match rule.as_str() {
                "stdout" => expected.stdout == actual.stdout,
                "stdout-lines" => sorted_lines(&expected.stdout) == sorted_lines(&actual.stdout),
                "stdout-trimmed" => trimmed(&expected.stdout) == trimmed(&actual.stdout),
                "status" => expected.status == actual.status,
                "status-class" => (expected.status == 0) == (actual.status == 0),
                "stderr-presence" => expected.stderr_empty == actual.stderr.is_empty(),
                _ => unreachable!(),
            };
            !ok
        })
        .cloned()
        .collect()
}

fn indent(text: &str) -> String {
    if text.is_empty() {
        return "    (empty)\n".to_string();
    }
    text.lines().map(|line| format!("    | {line}\n")).collect()
}

#[test]
fn differential() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "no cases in {}", cases_dir().display());

    if std::env::var_os("DIFFERENTIAL_RECORD").is_some() {
        for case in &cases {
            record(case);
            println!("recorded {}", case.name);
        }
        return;
    }

    let mut failures = Vec::new();
    let mut passed = 0;
    for case in &cases {
        if let Some(reason) = &case.known_difference {
            println!("skip {}: known difference: {reason}", case.name);
            continue;
        }
        let expected = parse_expected(case);
        let Some(actual) = run_strands(case) else {
            failures.push(format!("{}: timed out after {CASE_TIMEOUT:?}\n", case.name));
            continue;
        };
        let broken = check(&case.rules, &expected, &actual);
        if broken.is_empty() {
            passed += 1;
            continue;
        }
        failures.push(format!(
            "{}: failed rules: {}\n  expected (bash): status {}, stderr {}, stdout:\n{}  actual (strands-shell): status {}, stdout:\n{}  actual stderr:\n{}",
            case.name,
            broken.join(", "),
            expected.status,
            if expected.stderr_empty { "empty" } else { "non-empty" },
            indent(&expected.stdout),
            actual.status,
            indent(&actual.stdout),
            indent(&actual.stderr),
        ));
    }
    println!(
        "{passed} passed, {} failed, {} known differences",
        failures.len(),
        cases
            .iter()
            .filter(|c| c.known_difference.is_some())
            .count()
    );
    assert!(
        failures.is_empty(),
        "{} differential case(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}
