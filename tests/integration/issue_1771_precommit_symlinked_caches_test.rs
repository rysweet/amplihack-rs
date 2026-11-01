//! Issue #1771: run the symlinked pre-commit cache suite under `cargo test`.
//!
//! `scripts/git-hook-cargo.sh` is a shell script, and its cache sweep is
//! exercised by `tests/issue_1771_precommit_symlinked_caches.sh`. CI runs that
//! script in the Lint & Format job, but `cargo test --workspace` did not, so a
//! change to the sweep could pass the Rust suite unchecked. This test runs the
//! script and requires it to exit 0 with no failed case.
//!
//! The script builds every fixture under its own `mktemp -d` directory with a
//! fake `HOME`, and runs the wrapper around plain commands such as `printenv`
//! rather than a build, so it never touches the real home directory or a real
//! build cache. It runs in a few seconds.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const SUITE: &str = "tests/issue_1771_precommit_symlinked_caches.sh";

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

#[test]
fn symlinked_cache_suite_passes() {
    let root = workspace_root();
    let out = Command::new("bash")
        .arg(root.join(SUITE))
        .current_dir(&root)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap_or_else(|e| panic!("run {SUITE}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("failed: 0"),
        "{SUITE} failed ({}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        out.status
    );
}
