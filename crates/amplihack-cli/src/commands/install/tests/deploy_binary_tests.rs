//! Tests for `deploy_binary` — atomic rename-then-replace pattern that fixes
//! issue #304 (ETXTBSY when overwriting the running amplihack binary).

use super::super::filesystem::deploy_binary;
use std::fs;
use std::io;
use tempfile::TempDir;

#[test]
fn deploy_binary_replaces_existing_target() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-binary");
    let dst = tmp.path().join("dst-binary");
    fs::write(&src, b"new bytes").unwrap();
    fs::write(&dst, b"old bytes").unwrap();

    deploy_binary(&src, &dst).expect("deploy_binary");
    assert_eq!(fs::read(&dst).unwrap(), b"new bytes");
}

#[test]
fn deploy_binary_creates_missing_destination() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-binary");
    let dst = tmp.path().join("dst-binary");
    fs::write(&src, b"hello").unwrap();

    deploy_binary(&src, &dst).expect("deploy_binary");
    assert_eq!(fs::read(&dst).unwrap(), b"hello");
}

#[test]
#[cfg(unix)]
fn deploy_binary_sets_executable_mode() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-binary");
    let dst = tmp.path().join("dst-binary");
    fs::write(&src, b"data").unwrap();
    fs::set_permissions(&src, fs::Permissions::from_mode(0o644)).unwrap();

    deploy_binary(&src, &dst).expect("deploy_binary");
    let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "destination should be 0o755");
}

#[test]
fn deploy_binary_same_path_is_noop() {
    // Issue #302 / #304 interaction: when src and dst resolve to the same
    // file, deploy_binary returns Ok without touching the file. This guards
    // the legitimate re-stage-after-update workflow when src == dst.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("self");
    fs::write(&path, b"self bytes").unwrap();

    deploy_binary(&path, &path).expect("same-path deploy_binary");
    assert_eq!(fs::read(&path).unwrap(), b"self bytes");
}

#[test]
fn deploy_binary_cleans_up_temp_on_success() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::write(&src, b"x").unwrap();

    deploy_binary(&src, &dst).unwrap();

    // No leftover .new.* siblings in dst directory.
    let leftovers: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".new."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temp files should remain: {leftovers:?}"
    );
}

#[test]
fn deleted_source_with_existing_dst_is_noop() {
    // Issue #524: after `amplihack update` swaps the binary, the running
    // process's argv[0] points at a deleted inode. The auto-launched install
    // step then re-invokes deploy_binary with src == argv[0] (now ENOENT) and
    // dst == the freshly-written binary. This must succeed as a no-op since
    // the destination is already the correct, up-to-date binary.
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-deleted");
    let dst = tmp.path().join("dst-current");
    fs::write(&src, b"old/deleted bytes").unwrap();
    fs::write(&dst, b"freshly-installed bytes").unwrap();
    fs::remove_file(&src).unwrap();

    deploy_binary(&src, &dst).expect("deleted-source-with-dst should be Ok");
    assert_eq!(
        fs::read(&dst).unwrap(),
        b"freshly-installed bytes",
        "dst must remain untouched"
    );
}

#[test]
fn missing_source_and_missing_dst_returns_error() {
    // Guard rail for the #524 fix: if both src and dst are absent there is
    // nothing to deploy and no already-installed binary; propagate the
    // original ENOENT instead of silently masking a real failure.
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("nonexistent-src");
    let dst = tmp.path().join("nonexistent-dst");

    let err = deploy_binary(&src, &dst).expect_err("missing-both should error");
    let io_err = err
        .downcast_ref::<io::Error>()
        .expect("root cause should be io::Error");
    assert_eq!(
        io_err.kind(),
        io::ErrorKind::NotFound,
        "should propagate NotFound, got: {err:?}"
    );
}
