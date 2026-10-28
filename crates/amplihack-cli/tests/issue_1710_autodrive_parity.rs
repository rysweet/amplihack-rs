//! Issue #1710 (D8): auto-drive's state paths are known in exactly one Rust
//! module, `commands/recipe/autodrive_paths.rs`. The shell-parity check of
//! that module runs as a unit test beside it (`tests_autodrive_paths.rs`),
//! where its `pub(crate)` helpers are reachable; this file guards the other
//! half: no other Rust source spells the paths out. Read-only.

use std::fs;
use std::path::{Path, PathBuf};

const LITERALS: [&str; 6] = [
    "crusty-round-",
    "merge-ready-round-",
    "crusty-latest.json",
    "phases.tsv",
    "\"auto-drive\"",
    "/auto-drive",
];

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Production sources only: test files may name the files they create.
fn is_test_file(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("tests_") || n.ends_with("_tests.rs"))
}

#[test]
fn the_workstreams_module_exists_and_spells_out_no_auto_drive_path() {
    let dir = src_dir().join("commands").join("workstreams");
    let mut files = Vec::new();
    rust_files(&dir, &mut files);
    assert!(
        files.iter().any(|p| p.ends_with("mod.rs")),
        "commands/workstreams/mod.rs is missing"
    );
    for p in files.iter().filter(|p| !is_test_file(p)) {
        let text = fs::read_to_string(p).unwrap();
        for lit in LITERALS {
            assert!(
                !text.contains(lit),
                "{} spells out {lit}; use commands::recipe::autodrive_paths",
                p.display()
            );
        }
    }
}

#[test]
fn autodrive_paths_is_the_only_rust_module_that_names_round_files() {
    let mut files = Vec::new();
    rust_files(&src_dir(), &mut files);
    let owner = Path::new("commands/recipe/autodrive_paths.rs");
    assert!(
        files.iter().any(|p| p.ends_with(owner)),
        "commands/recipe/autodrive_paths.rs is missing"
    );
    let offenders: Vec<String> = files
        .iter()
        .filter(|p| !p.ends_with(owner) && !is_test_file(p))
        .filter(|p| {
            let text = fs::read_to_string(p).unwrap_or_default();
            ["crusty-round-", "merge-ready-round-", "crusty-latest.json"]
                .iter()
                .any(|lit| text.contains(lit))
        })
        .map(|p| p.display().to_string())
        .collect();
    assert!(offenders.is_empty(), "{offenders:?}");
}
