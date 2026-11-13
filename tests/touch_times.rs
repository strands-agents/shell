//! `touch` sets the access and modification times of host-bound and in-memory files.

#![cfg(unix)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strands_shell::Shell;

/// 2001-02-03T04:05:06Z.
const STAMP: u64 = 981_173_106;
/// 1990-01-01T00:00:00Z, a time no test sets.
const OLD: u64 = 631_152_000;

/// A writable bind at `/workspace` and a read-only bind at `/ro`, each seeded with one old file.
fn fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&base);
    let workspace = base.join("ws");
    let readonly = base.join("ro");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&readonly).expect("readonly");
    std::fs::write(workspace.join("f.txt"), "CONTENT").expect("file");
    std::fs::write(workspace.join("ref.txt"), "REF").expect("reference file");
    std::fs::write(readonly.join("ro.txt"), "RO").expect("read-only file");
    for file in [
        workspace.join("f.txt"),
        workspace.join("ref.txt"),
        readonly.join("ro.txt"),
    ] {
        set_host_times(&file, at(OLD), at(OLD));
    }
    (workspace, readonly)
}

fn build(workspace: &std::path::Path, readonly: &std::path::Path) -> Shell {
    let mut shell = Shell::builder()
        .bind_direct(workspace.to_str().expect("UTF-8"), "/workspace")
        .bind_direct_readonly(readonly.to_str().expect("UTF-8"), "/ro")
        .build()
        .expect("shell builds");
    shell.proc.cwd = std::path::PathBuf::from("/workspace");
    shell
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn set_host_times(path: &std::path::Path, accessed: SystemTime, modified: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open host file")
        .set_times(
            std::fs::FileTimes::new()
                .set_accessed(accessed)
                .set_modified(modified),
        )
        .expect("set host times");
}

/// The (access, modification) times of a host file.
fn host_times(path: &std::path::Path) -> (SystemTime, SystemTime) {
    let meta = std::fs::metadata(path).expect("stat host file");
    (
        meta.accessed().expect("access time"),
        meta.modified().expect("modification time"),
    )
}

async fn run(ws: &std::path::Path, ro: &std::path::Path, line: &str) -> strands_shell::Output {
    let mut sh = build(ws, ro);
    sh.run(line).await
}

#[tokio::test]
async fn touch_sets_an_existing_host_file_to_now_and_keeps_its_content() {
    let (ws, ro) = fixture("touch_now");
    let before = SystemTime::now() - Duration::from_secs(60);
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch f.txt"))
        .await;
    assert_eq!(out.status, 0, "touch must succeed: {}", out.stderr);
    let (accessed, modified) = host_times(&ws.join("f.txt"));
    assert!(modified > before, "the modification time is now");
    assert!(accessed > before, "the access time is now");
    assert_eq!(
        std::fs::read_to_string(ws.join("f.txt")).unwrap(),
        "CONTENT"
    );
}

#[tokio::test]
async fn touch_d_sets_both_host_times_to_the_date() {
    let (ws, ro) = fixture("touch_date");
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch -d 2001-02-03T04:05:06Z f.txt"))
        .await;
    assert_eq!(out.status, 0, "touch -d must succeed: {}", out.stderr);
    assert_eq!(host_times(&ws.join("f.txt")), (at(STAMP), at(STAMP)));
}

#[tokio::test]
async fn touch_m_t_changes_only_the_host_modification_time() {
    let (ws, ro) = fixture("touch_stamp_m");
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch -m -t 200102030405.06 f.txt"))
        .await;
    assert_eq!(out.status, 0, "touch -m -t must succeed: {}", out.stderr);
    assert_eq!(host_times(&ws.join("f.txt")), (at(OLD), at(STAMP)));
}

#[tokio::test]
async fn touch_a_changes_only_the_host_access_time() {
    let (ws, ro) = fixture("touch_stamp_a");
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch -a -d @981173106 f.txt"))
        .await;
    assert_eq!(out.status, 0, "touch -a must succeed: {}", out.stderr);
    assert_eq!(host_times(&ws.join("f.txt")), (at(STAMP), at(OLD)));
}

#[tokio::test]
async fn touch_r_copies_the_reference_host_times() {
    let (ws, ro) = fixture("touch_reference");
    set_host_times(&ws.join("ref.txt"), at(STAMP), at(STAMP + 60));
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch -r ref.txt f.txt"))
        .await;
    assert_eq!(out.status, 0, "touch -r must succeed: {}", out.stderr);
    assert_eq!(host_times(&ws.join("f.txt")), (at(STAMP), at(STAMP + 60)));
}

#[tokio::test]
async fn touch_creates_a_missing_host_file_at_the_date() {
    let (ws, ro) = fixture("touch_create");
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch -d 2001-02-03T04:05:06Z new.txt"))
        .await;
    assert_eq!(out.status, 0, "touch must succeed: {}", out.stderr);
    assert_eq!(std::fs::read(ws.join("new.txt")).unwrap(), b"");
    assert_eq!(host_times(&ws.join("new.txt")).1, at(STAMP));
}

#[tokio::test]
async fn touch_c_does_not_create_a_missing_host_file() {
    let (ws, ro) = fixture("touch_no_create");
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch -c missing.txt"))
        .await;
    assert_eq!(out.status, 0, "touch -c must succeed: {}", out.stderr);
    assert!(!ws.join("missing.txt").exists(), "no file was created");
}

#[tokio::test]
async fn touch_refuses_a_read_only_bind_and_leaves_its_times_unchanged() {
    let (ws, ro) = fixture("touch_readonly");
    let out = tokio::task::LocalSet::new()
        .run_until(run(&ws, &ro, "touch /ro/ro.txt f.txt"))
        .await;
    assert_eq!(out.status, 1, "a read-only file fails: {}", out.stderr);
    assert!(
        out.stderr.contains("/ro/ro.txt"),
        "names the file: {}",
        out.stderr
    );
    assert_eq!(host_times(&ro.join("ro.txt")), (at(OLD), at(OLD)));
    assert_ne!(
        host_times(&ws.join("f.txt")).1,
        at(OLD),
        "the next file is still touched"
    );
}

#[tokio::test]
async fn touch_sets_the_modification_time_of_an_in_memory_file() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut sh = Shell::builder().build().expect("shell builds");
            let out = sh
                .run(
                    "cd /tmp && echo hi > f && touch -d 2001-02-03T04:05:06Z f && ls -l f && cat f",
                )
                .await;
            assert_eq!(out.status, 0, "touch must succeed: {}", out.stderr);
            assert!(
                out.stdout.contains("Feb  3  2001"),
                "ls shows the date: {}",
                out.stdout
            );
            assert!(
                out.stdout.ends_with("hi\n"),
                "the content is kept: {}",
                out.stdout
            );
        })
        .await;
}

/// The (access, modification) times of an in-memory file.
async fn memory_times(sh: &Shell, path: &str) -> (Option<SystemTime>, Option<SystemTime>) {
    let stat = sh.kernel().stat(&sh.proc, path).await;
    assert!(stat.exists, "{path} exists");
    (stat.accessed, stat.modified)
}

#[tokio::test]
async fn touch_a_changes_only_the_access_time_of_an_in_memory_file() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut sh = Shell::builder()
                .build()
                .expect("shell builds");
            let out = sh
                .run("echo hi > /tmp/f && touch -d @631152000 /tmp/f && touch -a -d @981173106 /tmp/f")
                .await;
            assert_eq!(out.status, 0, "touch -a must succeed: {}", out.stderr);
            assert_eq!(
                memory_times(&sh, "/tmp/f").await,
                (Some(at(STAMP)), Some(at(OLD)))
            );
        })
        .await;
}

#[tokio::test]
async fn touch_r_copies_the_reference_times_of_an_in_memory_file() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut sh = Shell::builder().build().expect("shell builds");
            let out = sh
                .run(
                    "touch /tmp/ref /tmp/f && touch -m -d @631152000 /tmp/ref \
                     && touch -a -d @981173106 /tmp/ref && touch -r /tmp/ref /tmp/f",
                )
                .await;
            assert_eq!(out.status, 0, "touch -r must succeed: {}", out.stderr);
            assert_eq!(
                memory_times(&sh, "/tmp/f").await,
                (Some(at(STAMP)), Some(at(OLD)))
            );
        })
        .await;
}

#[tokio::test]
async fn touch_creates_a_missing_in_memory_file_at_the_date() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut sh = Shell::builder().build().expect("shell builds");
            let out = sh.run("touch -d @981173106 /tmp/new").await;
            assert_eq!(out.status, 0, "touch must succeed: {}", out.stderr);
            assert_eq!(
                memory_times(&sh, "/tmp/new").await,
                (Some(at(STAMP)), Some(at(STAMP)))
            );
        })
        .await;
}
