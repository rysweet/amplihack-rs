//! Issue #2022, AC1: no default time limit on work remains in the source.
//!
//! - The removed constants do not appear anywhere under `crates/` or
//!   `amplifier-bundle/`: `RECIPE_RUNNER_DEFAULT_TIMEOUT`,
//!   `CAPACITY_DEFAULT_MAX`, `DEFAULT_CLI_TIMEOUT`,
//!   `DEFAULT_IDLE_TIMEOUT_SECS` and the six-hour product `6 * 60 * 60`.
//! - `21600` / `21_600` appear only as explicit values in test files.
//! - The other work limits named in D7 are gone: `QUERY_TIMEOUT`,
//!   `DOCKER_BUILD_TIMEOUT`, `DOCKER_RUN_TIMEOUT`, `RUST_TRIAL_TIMEOUT`,
//!   `SCIP_INDEXER_TIMEOUT`, `SCOUT_REASONER_TIMEOUT`, `DEFAULT_MAX_RUNTIME`
//!   and the delegation `DEFAULT_TIMEOUT`.
//!
//! The banned names are assembled at run time so this file never matches
//! itself, and it is skipped by path as well.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

const SCANNED_EXTENSIONS: &[&str] = &["rs", "yaml", "yml", "sh", "toml", "py", "md", "json"];

/// Every scanned file under `dir`, skipping build output and VCS metadata.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                    continue;
                }
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| SCANNED_EXTENSIONS.contains(&e))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn scanned_files() -> Vec<PathBuf> {
    let root = repo_root();
    let me = Path::new(file!())
        .file_name()
        .expect("own file name")
        .to_os_string();
    let mut files = files_under(&root.join("crates"));
    files.extend(files_under(&root.join("amplifier-bundle")));
    files.retain(|p| p.file_name() != Some(me.as_os_str()));
    assert!(
        !files.is_empty(),
        "nothing scanned under {}",
        root.display()
    );
    files
}

/// `path:line: text` for each line containing `needle` as a whole word, so
/// a probe such as `TMUX_QUERY_TIMEOUT` does not match `QUERY_TIMEOUT`.
fn hits(needle: &str) -> Vec<String> {
    let re = regex::Regex::new(&format!(
        r"(?:^|[^A-Za-z0-9_]){}(?:$|[^A-Za-z0-9_])",
        regex::escape(needle)
    ))
    .expect("valid pattern");
    let mut found = Vec::new();
    for file in scanned_files() {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if re.is_match(line) {
                found.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    found
}

fn join(parts: &[&str]) -> String {
    parts.concat()
}

#[test]
fn ac1_removed_default_limit_constants_are_gone() {
    let banned = [
        join(&["RECIPE_RUNNER_", "DEFAULT_TIMEOUT"]),
        join(&["CAPACITY_", "DEFAULT_MAX"]),
        join(&["DEFAULT_", "CLI_TIMEOUT"]),
        join(&["DEFAULT_IDLE_", "TIMEOUT_SECS"]),
        join(&["6 * 60 ", "* 60"]),
    ];
    let mut all = Vec::new();
    for needle in &banned {
        all.extend(hits(needle));
    }
    assert!(
        all.is_empty(),
        "issue #2022: default time limits must be removed:\n  {}",
        all.join("\n  ")
    );
}

#[test]
fn d7_other_default_work_limits_are_gone() {
    let banned = [
        join(&["QUERY_", "TIMEOUT"]),
        join(&["DOCKER_BUILD_", "TIMEOUT"]),
        join(&["DOCKER_RUN_", "TIMEOUT"]),
        join(&["RUST_TRIAL_", "TIMEOUT"]),
        join(&["SCIP_INDEXER_", "TIMEOUT"]),
        join(&["SCOUT_REASONER_", "TIMEOUT"]),
        join(&["DEFAULT_MAX_", "RUNTIME"]),
    ];
    let mut all = Vec::new();
    for needle in &banned {
        all.extend(hits(needle));
    }
    let delegation = repo_root().join("crates/amplihack-delegation/src/state_machine.rs");
    let text = std::fs::read_to_string(&delegation).expect("delegation state machine");
    if text.contains(&join(&["const DEFAULT_", "TIMEOUT"])) {
        all.push(format!(
            "{}: the unused DEFAULT_TIMEOUT (1800 s) must be deleted",
            delegation.display()
        ));
    }
    assert!(
        all.is_empty(),
        "issue #2022: work limits listed in D7 must be removed:\n  {}",
        all.join("\n  ")
    );
}

/// A test file: under a `tests/` directory, or named `tests_*.rs`,
/// `*_test.rs` or `*_tests.rs`, or a `.sh` test under `recipes/tests/`.
fn is_test_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    path.components().any(|c| c.as_os_str() == "tests")
        || name.starts_with("tests_")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
}

#[test]
fn ac1_six_hours_in_seconds_appears_only_in_tests() {
    let mut outside = Vec::new();
    for needle in [join(&["216", "00"]), join(&["21_", "600"])] {
        for file in scanned_files() {
            if is_test_file(&file) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                if line.contains(&needle) {
                    outside.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        outside.is_empty(),
        "issue #2022: 21600 may appear only as an explicit value in tests:\n  {}",
        outside.join("\n  ")
    );
}

/// D6: the two bundled recipe limits on work are removed.
#[test]
fn d6_bundled_recipe_work_limits_are_removed() {
    let recipes = repo_root().join("amplifier-bundle/recipes");
    let quality_loop = std::fs::read_to_string(recipes.join("quality-loop.yaml")).unwrap();
    assert!(
        !quality_loop.contains("default_step_timeout:"),
        "quality-loop.yaml must not set default_step_timeout"
    );
    let audit = std::fs::read_to_string(recipes.join("quality-audit-cycle.yaml")).unwrap();
    assert!(
        !audit.contains(&join(&["timeout 900 ", "amplihack recipe run"])),
        "quality-audit-cycle.yaml must not wrap the recursive run in `timeout 900`"
    );
    assert!(
        audit.contains("recursion.max_depth") || audit.contains("max_depth"),
        "the Bug #646 comment must say recursion stays bounded by recursion.max_depth"
    );
    let quick_change = std::fs::read_to_string(recipes.join("quick-change.yaml")).unwrap();
    assert!(
        quick_change.contains("timeout 1800 bash -c"),
        "the gh pr create retry guard in quick-change.yaml is a network hang guard and stays"
    );
}

/// D4: the retry budget stays and says what it measures.
#[test]
fn d4_the_retry_budget_is_documented_as_backoff_only() {
    let retry = std::fs::read_to_string(
        repo_root().join("crates/amplihack-cli/src/commands/recipe/run/retry.rs"),
    )
    .unwrap();
    assert!(retry.contains("DEFAULT_BUDGET"), "the retry budget is kept");
    let lower = retry.to_lowercase();
    assert!(
        lower.contains("backoff") && lower.contains("never the time an attempt spends running"),
        "retry.rs must say the budget measures only the idle backoff between attempts, \
         never the time an attempt spends running"
    );
}
