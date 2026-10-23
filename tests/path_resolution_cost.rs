//! Resolving a missing path should cost roughly linear time in its component count.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use strands_shell::Shell;

const SHORT: usize = 2_000;
const LONG: usize = 8 * SHORT;

fn shell(name: &str) -> Shell {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&base);
    let workspace = base.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut shell = Shell::builder()
        .bind_direct(workspace.to_str().unwrap(), "/workspace")
        .max_input(1 << 20)
        .build()
        .unwrap();
    shell.proc.cwd = PathBuf::from("/workspace");
    shell
}

async fn time_to_fail(name: &str, root: &str, components: usize) -> Duration {
    let mut shell = shell(name);
    let path = vec!["a"; components].join("/");
    let start = Instant::now();
    let output = shell.run(&format!("cat -- {root}/{path}")).await;
    let elapsed = start.elapsed();
    assert_ne!(
        output.status, 0,
        "a missing path was read: {}",
        output.stdout
    );
    elapsed
}

// Eight times the components should take about eight times as long; a quadratic walk takes
// about sixty-four. Comparing two sizes in one run keeps the bound independent of machine speed.
async fn assert_grows_linearly(root: &str) {
    let short = time_to_fail("path-cost-short", root, SHORT).await;
    let long = time_to_fail("path-cost-long", root, LONG).await;
    let bound = short * 24 + Duration::from_secs(2);
    assert!(
        long < bound,
        "{LONG} components under {root:?} took {long:?}, {SHORT} took {short:?}"
    );
}

#[tokio::test]
async fn a_missing_path_of_many_components_resolves_in_linear_time() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            assert_grows_linearly("/workspace").await;
            assert_grows_linearly("").await;
        })
        .await;
}
