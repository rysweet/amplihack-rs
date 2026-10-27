//! Issue #1658 (FS-1, FS-2): source guards for the limit-resume hook. It
//! makes directories one level at a time, never changes an existing path's
//! mode and never prints.
//!
//! The hook turns a panic into a silent exit 0 with `catch_unwind`, which
//! only works while release builds unwind.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap_or_else(|_| panic!("{} must exist", d.display())) {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("tests")
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn scanned_dirs() -> [PathBuf; 2] {
    let ws = workspace();
    [
        ws.join("crates/amplihack-hooks/src/limit_resume"),
        ws.join("crates/amplihack-utils/src/rotator_status"),
    ]
}

#[test]
fn hook_sources_never_use_create_dir_all_or_chmod() {
    // FS-1/FS-2: directories are made one level at a time and an existing
    // path's mode is never changed.
    for dir in scanned_dirs() {
        for path in rust_sources(&dir) {
            let text = fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                for f in [
                    "create_dir_all",
                    "set_permissions",
                    "ensure_private_directory",
                    "open_private_append",
                    "println!",
                    "print!(",
                ] {
                    assert!(
                        !code.contains(f),
                        "{}:{}: `{f}`: {line}",
                        path.display(),
                        i + 1
                    );
                }
            }
        }
    }
}

#[test]
fn release_builds_unwind_so_catch_unwind_works() {
    let manifest = fs::read_to_string(workspace().join("Cargo.toml")).unwrap();
    let release = manifest
        .split("[profile.release]")
        .nth(1)
        .expect("workspace has a [profile.release]");
    let section = release.split("\n[").next().unwrap();
    assert!(
        section.contains("panic = \"unwind\""),
        "limit-resume relies on catch_unwind; keep panic = \"unwind\""
    );
}
