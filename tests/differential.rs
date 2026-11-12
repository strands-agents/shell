//! Differential tests: each case in `tests/differential/cases/` is a small
//! shell script whose `.expected` file was recorded from bash. This test runs
//! every case through strands-shell and compares the result using the rules
//! named in the case header. See `tests/differential/README.md`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use strands_shell::{Output, Shell};

const DEFAULT_RULES: &[&str] = &["stdout", "status", "stderr-presence"];
const CASE_TIMEOUT: Duration = Duration::from_secs(30);
const RECORD_PATH: &str = "/usr/bin:/bin";

struct Case {
    name: String,
    path: PathBuf,
    script: String,
    rules: Vec<String>,
    known_difference: Option<String>,
    skip: Option<String>,
}

struct Expected {
    status: i32,
    stderr_empty: bool,
    stdout: String,
}

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential/cases")
}

/// Reads a file byte for byte. `.gitattributes` keeps line endings in the
/// cases directory unconverted, so output containing `\r` survives checkout.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
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
    let script = read(path);
    let mut rules: Vec<String> = DEFAULT_RULES.iter().map(|r| r.to_string()).collect();
    let mut known_difference = None;
    let mut skip = None;
    for line in script.lines().take_while(|line| line.starts_with('#')) {
        let line = line.trim_start_matches('#').trim();
        if let Some(list) = line.strip_prefix("compare:") {
            rules = list.split(',').map(|r| r.trim().to_string()).collect();
        } else if let Some(reason) = line.strip_prefix("known-difference:") {
            known_difference = Some(reason.trim().to_string());
        } else if let Some(reason) = line.strip_prefix("skip:") {
            skip = Some(reason.trim().to_string());
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
        skip,
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
    let text = read(&path);
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

/// Runs the case under bash in a fresh directory with a fixed environment
/// and returns its exit status and `.expected` text.
fn record(case: &Case) -> (i32, String) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("differential")
        .join(&case.name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new("bash")
        .arg(&case.path)
        .current_dir(&dir)
        .env_clear()
        .env("PATH", RECORD_PATH)
        .env("HOME", &dir)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .expect("run bash");
    std::fs::remove_dir_all(&dir).unwrap();
    let status = out.status.code().expect("bash killed by signal");
    let stdout = String::from_utf8(out.stdout).expect("bash stdout is UTF-8");
    (
        status,
        format_expected(status, out.stderr.is_empty(), &stdout),
    )
}

/// Records every case. Writes nothing if bash is missing or any case exits
/// 127, which usually means a command it uses isn't installed.
fn record_all(cases: &[Case]) {
    let bash = Command::new("bash")
        .args(["-c", "true"])
        .env_clear()
        .env("PATH", RECORD_PATH)
        .status();
    assert!(
        bash.is_ok_and(|s| s.success()),
        "recording needs bash on {RECORD_PATH}"
    );
    let recorded: Vec<(&Case, i32, String)> = cases
        .iter()
        .map(|case| {
            let (status, text) = record(case);
            (case, status, text)
        })
        .collect();
    let missing: Vec<&str> = recorded
        .iter()
        .filter(|(_, status, _)| *status == 127)
        .map(|(case, _, _)| case.name.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "nothing recorded: these cases exited 127, probably a tool missing from {RECORD_PATH}: {}",
        missing.join(", ")
    );
    for (case, _, text) in recorded {
        std::fs::write(expected_path(case), text).unwrap();
        println!("recorded {}", case.name);
    }
}

/// Runs the case in a fresh shell on its own thread, so a hang fails the case
/// instead of blocking the whole test.
fn run_strands(case: &Case) -> Result<Output, String> {
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
    rx.recv_timeout(CASE_TIMEOUT).map_err(|e| match e {
        RecvTimeoutError::Timeout => format!("timed out after {CASE_TIMEOUT:?}"),
        RecvTimeoutError::Disconnected => "panicked".to_string(),
    })
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

/// Shows each line escaped, so `\r`, trailing spaces and a missing final
/// newline are visible.
fn indent(text: &str) -> String {
    if text.is_empty() {
        return "    (empty)\n".to_string();
    }
    text.split_inclusive('\n')
        .map(|line| format!("    {line:?}\n"))
        .collect()
}

/// Runs the case and describes how it differs from bash, or `None` if it
/// matches.
fn difference(case: &Case) -> Option<String> {
    let expected = parse_expected(case);
    let actual = match run_strands(case) {
        Ok(actual) => actual,
        Err(e) => return Some(e),
    };
    let broken = check(&case.rules, &expected, &actual);
    if broken.is_empty() {
        return None;
    }
    Some(format!(
        "failed rules: {}\n  expected (bash): status {}, stderr {}, stdout:\n{}  actual (strands-shell): status {}, stdout:\n{}  actual stderr:\n{}",
        broken.join(", "),
        expected.status,
        if expected.stderr_empty {
            "empty"
        } else {
            "non-empty"
        },
        indent(&expected.stdout),
        actual.status,
        indent(&actual.stdout),
        indent(&actual.stderr),
    ))
}

#[test]
fn differential() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "no cases in {}", cases_dir().display());

    if std::env::var_os("DIFFERENTIAL_RECORD").is_some() {
        record_all(&cases);
        return;
    }

    let mut failures = Vec::new();
    let mut passed = 0;
    let mut known = 0;
    for case in &cases {
        if let Some(reason) = &case.skip {
            println!("skip {}: {reason}", case.name);
            continue;
        }
        match (&case.known_difference, difference(case)) {
            (None, None) => passed += 1,
            (None, Some(diff)) => failures.push(format!("{}: {diff}", case.name)),
            (Some(reason), Some(_)) => {
                known += 1;
                println!("known difference {}: {reason}", case.name);
            }
            (Some(_), None) => failures.push(format!(
                "{} now matches bash: remove the known-difference marker\n",
                case.name
            )),
        }
    }
    println!(
        "{passed} passed, {} failed, {known} known differences",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} differential case(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}
