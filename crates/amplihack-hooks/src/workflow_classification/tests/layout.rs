//! The layout of the workflow classification module (review of PR #1752).
//!
//! While issue #1742 was built, `workflow_classification.rs` grew to 1,564
//! lines, 1,094 of them inline tests. It is now `workflow_classification.rs`
//! plus `coordination.rs` and `state.rs`, with the unit tests under
//! `workflow_classification/tests`. These checks hold it to the crate's line
//! budget, the same numbers as `tests/issue_1483_memory_module_layout.rs`.

use std::fs;
use std::path::{Path, PathBuf};

/// A production file must be shorter than this.
const BRICK_LIMIT: usize = 400;

/// A test file must be shorter than this.
const TEST_FILE_LIMIT: usize = 600;

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The `.rs` files directly inside `dir`, sorted.
fn rust_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    files
}

/// `workflow_classification.rs` and every production file beside `tests.rs`.
fn production_files() -> Vec<PathBuf> {
    let mut files = vec![src().join("workflow_classification.rs")];
    files.extend(
        rust_files_in(&src().join("workflow_classification"))
            .into_iter()
            .filter(|path| !path.ends_with("tests.rs")),
    );
    files
}

fn test_files() -> Vec<PathBuf> {
    let mut files = vec![src().join("workflow_classification/tests.rs")];
    files.extend(rust_files_in(&src().join("workflow_classification/tests")));
    files
}

#[test]
fn production_files_are_under_the_brick_budget() {
    let files = production_files();
    assert!(files.len() >= 3, "the module and its two parts: {files:?}");
    for path in files {
        let lines = read(&path).lines().count();
        assert!(
            lines < BRICK_LIMIT,
            "{} has {lines} lines; the brick budget is {BRICK_LIMIT}",
            path.display()
        );
    }
}

/// Tests and test-only helpers live under `workflow_classification/tests`.
/// The only `#[cfg(test)]` item a production file may have is `mod tests;`.
#[test]
fn production_files_hold_no_tests() {
    for path in production_files() {
        let body = read(&path);
        assert!(
            !body.contains("#[test]"),
            "{} contains a test; move it under workflow_classification/tests",
            path.display()
        );
        let lines: Vec<&str> = body.lines().map(str::trim).collect();
        for (i, line) in lines.iter().enumerate() {
            if *line == "#[cfg(test)]" {
                assert_eq!(
                    lines.get(i + 1).copied(),
                    Some("mod tests;"),
                    "{} line {}: a test-only item in a production file",
                    path.display(),
                    i + 1
                );
            }
        }
    }
}

#[test]
fn test_files_are_under_the_test_file_budget() {
    let files = test_files();
    assert!(files.len() > 1, "the tests are split by area: {files:?}");
    for path in files {
        let lines = read(&path).lines().count();
        assert!(
            lines < TEST_FILE_LIMIT,
            "{} has {lines} lines; the test file budget is {TEST_FILE_LIMIT}",
            path.display()
        );
    }
}
