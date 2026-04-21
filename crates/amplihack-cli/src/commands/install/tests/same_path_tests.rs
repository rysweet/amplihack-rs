//! Tests for the same-path skip behavior in `copy_dir_recursive` (issue #302).

use super::super::filesystem::copy_dir_recursive;
use std::fs;
use tempfile::TempDir;

#[test]
fn copy_dir_recursive_skips_same_path() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("d");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("file"), b"contents").unwrap();

    // Same path on both sides — must succeed (no error) and leave file intact.
    copy_dir_recursive(&dir, &dir).expect("same-path copy should be a no-op");
    assert_eq!(fs::read(dir.join("file")).unwrap(), b"contents");
}

#[test]
fn copy_dir_recursive_copies_distinct_paths_normally() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a"), b"hello").unwrap();
    fs::create_dir_all(src.join("nested")).unwrap();
    fs::write(src.join("nested").join("b"), b"world").unwrap();

    copy_dir_recursive(&src, &dst).expect("distinct-path copy");

    assert_eq!(fs::read(dst.join("a")).unwrap(), b"hello");
    assert_eq!(fs::read(dst.join("nested").join("b")).unwrap(), b"world");
}
