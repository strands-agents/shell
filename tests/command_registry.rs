//! The registered commands are listed in several places that must agree:
//! the native registry behind `strands-shell list-commands`, the static
//! lookup table WASM builds use in `src/commands/mod.rs`, the `/bin` links
//! WASM builds create in `src/vfs.rs`, and the command count in the README.
//! WASM can't be built here, so the WASM lists are read from the source.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use strands_shell::Shell;

/// Registered commands that WASM builds don't link into `/bin`.
const WASM_BIN_OMITS: &[&str] = &["curl", "sleep"];

fn source(path: &str) -> String {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {}: {e}", full.display()))
}

/// The quoted names from the start of `text` up to the first `]`.
fn quoted_names(text: &str) -> Vec<String> {
    let list = &text[..text.find(']').expect("unterminated list")];
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn list_commands_output() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_strands-shell"))
        .arg("list-commands")
        .output()
        .expect("run strands-shell list-commands");
    assert!(out.status.success(), "list-commands failed: {out:?}");
    assert!(
        out.stderr.is_empty(),
        "list-commands wrote to stderr: {out:?}"
    );
    String::from_utf8(out.stdout).expect("list-commands output is UTF-8")
}

fn native_commands() -> Vec<String> {
    list_commands_output().lines().map(str::to_string).collect()
}

/// The names matched by the WASM `lookup` table in `src/commands/mod.rs`.
fn wasm_lookup_table() -> Vec<String> {
    let text = source("src/commands/mod.rs");
    let start = text
        .find("let func: CommandFn = match name {")
        .expect("WASM lookup table in src/commands/mod.rs");
    let table = &text[start..];
    let table = &table[..table.find("_ => return None").expect("end of lookup table")];
    table
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix('"')?;
            let (name, arm) = rest.split_once('"')?;
            arm.trim_start().starts_with("=>").then(|| name.to_string())
        })
        .collect()
}

/// The names in the WASM `/bin` list in `create_bin_links` in `src/vfs.rs`.
fn wasm_bin_links() -> Vec<String> {
    let text = source("src/vfs.rs");
    let start = text
        .find("Create /bin symlinks for all known commands on WASM")
        .expect("WASM /bin list in src/vfs.rs");
    let text = &text[start..];
    let text = &text[text.find("let cmds = [").expect("cmds list") + "let cmds = [".len()..];
    quoted_names(text)
}

fn set(names: &[String]) -> BTreeSet<&str> {
    names.iter().map(String::as_str).collect()
}

fn duplicates(names: &[String]) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    names
        .iter()
        .filter(|name| !seen.insert(name.as_str()))
        .map(String::as_str)
        .collect()
}

#[test]
fn list_commands_is_sorted_unique_and_has_known_commands() {
    let names = native_commands();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        names, sorted,
        "list-commands output is not sorted and unique"
    );
    for known in ["cat", "curl", "date", "grep", "jq", "ls", "sed", "wc"] {
        assert!(
            names.iter().any(|n| n == known),
            "{known} missing: {names:?}"
        );
    }
    // Builtins live in src/builtins and aren't registered commands.
    for builtin in ["cd", "export", "find", "printf", "test"] {
        assert!(
            !names.iter().any(|n| n == builtin),
            "builtin {builtin} listed as a command"
        );
    }
}

#[test]
fn wasm_lookup_table_matches_the_native_registry() {
    let native = native_commands();
    let wasm = wasm_lookup_table();
    assert!(
        duplicates(&wasm).is_empty(),
        "duplicate names in the WASM lookup table: {:?}",
        duplicates(&wasm)
    );
    assert_eq!(
        set(&wasm),
        set(&native),
        "the WASM lookup table in src/commands/mod.rs and the #[command] registry disagree"
    );
}

#[test]
fn wasm_bin_links_cover_every_command_but_the_known_omissions() {
    let wasm = wasm_lookup_table();
    let bin = wasm_bin_links();
    assert!(
        duplicates(&bin).is_empty(),
        "duplicate names in the WASM /bin list: {:?}",
        duplicates(&bin)
    );
    let mut expected = set(&wasm);
    for omitted in WASM_BIN_OMITS {
        assert!(
            expected.remove(omitted),
            "{omitted} is no longer a command; drop it from WASM_BIN_OMITS"
        );
    }
    assert_eq!(
        set(&bin),
        expected,
        "the WASM /bin list in src/vfs.rs should hold every command in the WASM lookup \
         table except {WASM_BIN_OMITS:?} (if you linked one of those, drop it from WASM_BIN_OMITS)"
    );
}

#[test]
fn native_bin_links_every_registered_command() {
    let native = native_commands();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    let out = rt.block_on(local.run_until(async {
        let mut shell = Shell::builder().build().unwrap();
        shell.run("ls /bin").await
    }));
    assert_eq!(out.status, 0, "ls /bin: {}", out.stderr);
    let bin: BTreeSet<&str> = out.stdout.lines().collect();
    for name in &native {
        assert!(bin.contains(name.as_str()), "/bin/{name} missing");
    }
}

#[test]
fn readme_command_count_matches_the_registry() {
    let count = native_commands().len();
    let readme = source("README.md");
    let claims: Vec<&str> = readme
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|pair| pair[1].trim_end_matches([',', '.', '"']) == "commands")
        .map(|pair| pair[0])
        .filter(|n| n.chars().all(|c| c.is_ascii_digit()))
        .collect();
    assert!(!claims.is_empty(), "README states no command count");
    for claim in claims {
        assert_eq!(
            claim,
            count.to_string(),
            "README says {claim} commands, list-commands shows {count}"
        );
    }
}
