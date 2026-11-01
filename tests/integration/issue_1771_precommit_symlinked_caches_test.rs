//! Issue #1771: run the symlinked pre-commit cache suite under `cargo test`.
//!
//! `scripts/git-hook-cargo.sh` is a shell script, and its cache sweep is
//! exercised by `tests/issue_1771_precommit_symlinked_caches.sh`. CI runs that
//! script in the Lint & Format job, but `cargo test --workspace` did not, so a
//! change to the sweep could pass the Rust suite unchecked. This test runs the
//! script and requires it to exit 0 with no failed case.
//!
//! `bash` is started through the shared [`isolated`] helper, as every spawned
//! test command must be (`tests/support/isolated_amplihack.rs`). The helper
//! clears the environment and keeps only `PATH`, `TMPDIR`, the locale and a
//! few other basic variables, so `XDG_*`, `GIT_*` (including `GIT_DIR`,
//! `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_PARAMETERS`) and `AMPLIHACK_*` from the
//! developer's shell do not reach the suite. It also sets the home guard and
//! `GIT_TERMINAL_PROMPT=0` with a null stdin. On top of that, this test points
//! `HOME` and `TMPDIR` at a temporary directory of its own, so the suite's
//! outer `git init` and its HOME checks never read the developer's real home
//! directory or git configuration. Each case inside the script then gets its
//! own fake `HOME` under that directory, and the wrapper runs plain commands
//! such as `printenv` rather than a build. It runs in a few seconds.

use std::path::PathBuf;

use crate::isolated_amplihack::isolated;

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
    let tmp = tempfile::TempDir::new().expect("create the suite temp dir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("create the suite HOME");
    let out = isolated("bash")
        .arg(root.join(SUITE))
        .current_dir(&root)
        .env("HOME", &home)
        .env("TMPDIR", tmp.path())
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
