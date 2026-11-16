//! Issue #2712: a recipe shell test suite must not put its worktrees in the
//! work root it inherited from the recipe run that started it.
//!
//! A recipe run exports `AMPLIHACK_WORK_ROOT` and
//! `AMPLIHACK_WORK_ROOT_EXPLICIT=1`, and every process it starts inherits
//! them, including the pre-commit and pre-push suites. With both set,
//! `workflow_worktree_root.sh base` puts a new worktree under
//! `<work root>/worktrees/<name>-<hash8>` instead of under `<main>/worktrees`.
//! `test-default-workflow-reliability.sh` builds its fixture repositories
//! inside its own scratch directory and removes only that directory in its
//! `EXIT` trap, so the worktrees stayed behind with a `.git` file pointing at
//! nothing. 255 of the 259 orphans swept from the shared work root on 15
//! November came from that shape.
//!
//! The fix is one sourced setup fragment,
//! `amplifier-bundle/recipes/tests/recipe-test-env.sh`, which clears the
//! inherited work root, plus `isolated()` for the Rust tests that start a
//! suite. These tests hold both halves:
//!
//! - [`outside_work_root_stays_empty`] is the behaviour test the issue asks
//!   for. It runs the suite that actually leaked, with an explicit work root
//!   supplied from outside, and requires the suite to pass and that directory
//!   to stay empty. It takes 30-45 s, which is the cost of covering the suite
//!   that produced the orphans rather than one that was already immune.
//! - [`recipe_test_env_is_source_only`] and
//!   [`recipe_test_env_clears_the_inherited_work_root`] are sub-second unit
//!   tests of the fragment itself: its file mode, and what sourcing it does
//!   and does not change. They look at the new file only; neither scans the
//!   suites for a phrase.
//!
//! `AMPLIHACK_HOME` is set deliberately in the behaviour test and left alone
//! by the fragment. Step 04 of `workflow-worktree.yaml` `cd`s into the
//! repository under test and then resolves its helpers through a ladder whose
//! first tier is `${AMPLIHACK_HOME}/amplifier-bundle/tools`. Without that
//! variable the ladder finds no `workflow_worktree_root.sh`, never asks for
//! the explicit base, and the leak does not reproduce at all - which is why a
//! run in a bare environment looks deceptively clean.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::isolated_amplihack::isolated;

/// The suite that leaked: it executes the extracted step-04 body with an
/// inherited environment.
const SUITE: &str = "amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh";

/// The shared setup fragment the suites source.
const FRAGMENT: &str = "amplifier-bundle/recipes/tests/recipe-test-env.sh";

/// The names the fragment clears.
const CLEARED: [&str; 4] = [
    "AMPLIHACK_WORK_ROOT",
    "AMPLIHACK_WORK_ROOT_EXPLICIT",
    "AMPLIHACK_RUN_DIR",
    "AMPLIHACK_RUN_ID",
];

/// A name the fragment must leave alone: the suites' step bodies resolve their
/// helpers through it.
const KEPT: &str = "AMPLIHACK_HOME";

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

/// Paths under `root`, to `depth` levels, as absolute path strings: the roots
/// are built from a `TempDir`, so every entry is reported in full.
///
/// Symbolic links are listed and never followed: `symlink_metadata` reports
/// the link itself, so a link planted in the work root cannot steer the walk
/// somewhere else. Nothing here removes or canonicalises anything.
///
/// The root and the descent below it fail differently on purpose. `root` is
/// created by the test itself, so a `read_dir` failure there is a broken run
/// rather than an empty directory, and returning the empty set would reach
/// the caller as "nothing leaked" and pass; so it panics. Below the root an
/// entry can legitimately disappear between the `read_dir` and the stat, so
/// those failures are skipped.
fn entries_under(root: &Path, depth: usize) -> BTreeSet<String> {
    let dir =
        fs::read_dir(root).unwrap_or_else(|e| panic!("read the work root {}: {e}", root.display()));
    entries_from(dir, depth)
}

/// The lenient descent. See `entries_under` for why it tolerates what the
/// root does not.
fn entries_from(dir: fs::ReadDir, depth: usize) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for entry in dir.flatten() {
        let path = entry.path();
        found.insert(path.display().to_string());
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_dir() || depth <= 1 {
            continue;
        }
        if let Ok(sub) = fs::read_dir(&path) {
            found.extend(entries_from(sub, depth - 1));
        }
    }
    found
}

/// The behaviour test issue #2712 asks for: one worktree-creating suite, an
/// explicit work root inherited from outside, and nothing left under it.
///
/// Both conditions are reported together. A revert produces both symptoms -
/// the suite fails its `${case_dir}/worktrees/<branch>` assertion *and* the
/// worktree appears in the outside root - and the failure message should say
/// which, so the exit status is not asserted first and allowed to mask the
/// leak.
#[test]
fn outside_work_root_stays_empty() {
    let root = workspace_root();
    let suite = root.join(SUITE);
    assert!(suite.is_file(), "missing {}", suite.display());

    // tempfile: a fresh random name, mode 0700, owned by this user, removed on
    // drop. Never a fixed path under /tmp, and nothing here deletes a path
    // assembled from the suite's output.
    let tmp = tempfile::TempDir::new().expect("create the test temp dir");
    let outside = tmp.path().join("work-root");
    let home = tmp.path().join("home");
    let scratch = tmp.path().join("tmp");
    let run_dir = tmp.path().join("run");
    for dir in [&outside, &home, &scratch, &run_dir] {
        fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    }

    let out = isolated("bash")
        .arg(&suite)
        .current_dir(&root)
        .env("HOME", &home)
        .env("TMPDIR", &scratch)
        // What a recipe run gives every process it starts. AMPLIHACK_HOME is
        // what makes step 04 find the worktree helpers at all; the other four
        // are what the fragment must clear.
        .env(KEPT, &root)
        .env("AMPLIHACK_WORK_ROOT", &outside)
        .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        .env("AMPLIHACK_RUN_DIR", &run_dir)
        .env("AMPLIHACK_RUN_ID", "issue-2712-behaviour-test")
        .output()
        .unwrap_or_else(|e| panic!("run {SUITE}: {e}"));

    let leaked = entries_under(&outside, 4);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    let mut problems = Vec::new();
    if !out.status.success() {
        problems.push(format!(
            "{SUITE} exited {} with an inherited explicit work root; it must pass unchanged",
            out.status
        ));
    }
    if !leaked.is_empty() {
        problems.push(format!(
            "{SUITE} left {} path(s) under the inherited work root {}; its worktrees must stay in its own scratch directory:\n  {}",
            leaked.len(),
            outside.display(),
            leaked.into_iter().collect::<Vec<_>>().join("\n  ")
        ));
    }
    assert!(
        problems.is_empty(),
        "issue #2712: {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        problems.join("\n")
    );
}

/// The fragment is sourced, never executed: no execute bit and no shebang.
///
/// CI names each suite in its own step rather than scanning the directory, so
/// the file cannot be picked up as a suite; the absent execute bit removes the
/// remaining way it could be run by accident.
#[test]
fn recipe_test_env_is_source_only() {
    let path = workspace_root().join(FRAGMENT);
    let meta = fs::symlink_metadata(&path)
        .unwrap_or_else(|e| panic!("stat {}: {e} (issue #2712)", path.display()));
    assert!(
        meta.file_type().is_file(),
        "{} must be a regular file",
        path.display()
    );
    let mode = meta.permissions().mode() & 0o7777;
    assert_eq!(
        mode & 0o111,
        0,
        "{} is executable (mode {mode:04o}); it is sourced, never run",
        path.display()
    );
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        !text.starts_with("#!"),
        "{} starts with a shebang; it is sourced, never run",
        path.display()
    );
}

/// Sourcing the fragment clears the four inherited names, leaves
/// `AMPLIHACK_HOME` alone, prints nothing, and is safe to source twice.
///
/// The variables are supplied through the child's environment rather than
/// exported inside the script, which is how a suite really receives them.
#[test]
fn recipe_test_env_clears_the_inherited_work_root() {
    let root = workspace_root();
    let path = root.join(FRAGMENT);
    assert!(path.is_file(), "missing {} (issue #2712)", path.display());

    // `set -u` is on, as in every suite: an unset name must still be readable
    // through the `+set` form, and sourcing twice must not fail.
    let script = format!(
        r#"set -euo pipefail
. "$1"
. "$1"
for name in {names} {kept}; do
    if [ -n "${{!name+set}}" ]; then
        printf '%s=set\n' "$name"
    else
        printf '%s=unset\n' "$name"
    fi
done
"#,
        names = CLEARED.join(" "),
        kept = KEPT,
    );

    let out = isolated("bash")
        .arg("-c")
        .arg(&script)
        .arg("recipe-test-env-probe")
        .arg(&path)
        .current_dir(&root)
        .env(KEPT, &root)
        .env("AMPLIHACK_WORK_ROOT", "/nonexistent/shared/work-root")
        .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        .env("AMPLIHACK_RUN_DIR", "/nonexistent/runs/issue-2712")
        .env("AMPLIHACK_RUN_ID", "issue-2712-unit")
        .output()
        .unwrap_or_else(|e| panic!("source {}: {e}", path.display()));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "sourcing {} exited {}:\n{stderr}",
        path.display(),
        out.status
    );

    let mut expected: Vec<String> = CLEARED.iter().map(|n| format!("{n}=unset")).collect();
    expected.push(format!("{KEPT}=set"));
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        expected,
        "sourcing {} must clear exactly {CLEARED:?}, leave {KEPT} alone, and print nothing of its own",
        path.display()
    );
    assert!(
        stderr.is_empty(),
        "sourcing {} wrote to stderr: {stderr}",
        path.display()
    );
}
