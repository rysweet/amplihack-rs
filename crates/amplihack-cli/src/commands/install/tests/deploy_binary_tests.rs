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

// ── stage_binary (issue #1954) ────────────────────────────────────────
//
// `deploy_binary` above is legacy code kept for these tests only; production
// installs call `stage_binary`, which copies into a release and switches a
// link. These tests carry the #302, #524 and #885 rules over to it.

#[cfg(unix)]
mod stage_binary {
    use super::super::super::binary::stage_binary;
    use super::super::super::release_layout::{ReleaseTxn, releases_dir};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// `<tmp>/.amplihack` and `<tmp>/bin`.
    fn dirs() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let amp = tmp.path().join(".amplihack");
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&amp).unwrap();
        fs::create_dir_all(&bin).unwrap();
        (tmp, amp, bin)
    }

    fn release_count(amp: &Path) -> usize {
        fs::read_dir(releases_dir(amp))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .count()
            })
            .unwrap_or(0)
    }

    /// #524: an update replaced the running binary, so `src` (argv[0]) is
    /// gone while `dst` holds the fresh install. Nothing is staged and `dst`
    /// is left exactly as it was.
    #[test]
    fn deleted_source_with_existing_dst_is_noop() {
        let (_tmp, amp, bin) = dirs();
        let src = bin.join("src-deleted");
        let dst = bin.join("amplihack");
        fs::write(&src, b"old/deleted bytes").unwrap();
        fs::write(&dst, b"freshly-installed bytes").unwrap();
        fs::remove_file(&src).unwrap();

        let mut txn = ReleaseTxn::allocate(&amp, "1.0.0").unwrap();
        let staged = stage_binary(&mut txn, &src, &dst).expect("deleted source with dst is Ok");
        assert!(!staged, "nothing may be staged for a deleted source");
        let report = txn.commit().unwrap();

        assert!(report.switched.is_empty(), "no link may be switched");
        assert!(report.failed.is_none());
        let meta = fs::symlink_metadata(&dst).unwrap();
        assert!(meta.is_file(), "dst must stay the installed regular file");
        assert_eq!(fs::read(&dst).unwrap(), b"freshly-installed bytes");
    }

    /// Guard rail for #524: with neither `src` nor `dst` present there is
    /// nothing to keep, so the original `NotFound` propagates and the
    /// unpublished release is removed.
    #[test]
    fn missing_source_and_missing_dst_returns_error() {
        let (_tmp, amp, bin) = dirs();
        let src = bin.join("nonexistent-src");
        let dst = bin.join("amplihack");

        let mut txn = ReleaseTxn::allocate(&amp, "1.0.0").unwrap();
        let err = stage_binary(&mut txn, &src, &dst).expect_err("missing-both should error");
        let io_err = err
            .downcast_ref::<io::Error>()
            .expect("root cause should be io::Error");
        assert_eq!(
            io_err.kind(),
            io::ErrorKind::NotFound,
            "should propagate NotFound, got: {err:?}"
        );
        drop(txn);

        assert!(fs::symlink_metadata(&dst).is_err(), "dst must not appear");
        assert_eq!(release_count(&amp), 0, "the unpublished release is removed");
    }

    /// #302 / #885: re-staging the binary that is already installed. `src`
    /// is the file `dst` names (the running executable resolves through the
    /// link into its release). The old release file must not be truncated or
    /// rewritten; the new release holds a copy and `dst` names it.
    #[test]
    fn source_that_is_the_dst_release_target_is_copied_not_rewritten() {
        let (_tmp, amp, bin) = dirs();
        let built = bin.join("built");
        fs::write(&built, b"self bytes").unwrap();
        let dst = bin.join("amplihack");

        let mut first = ReleaseTxn::allocate(&amp, "1.0.0").unwrap();
        assert!(stage_binary(&mut first, &built, &dst).unwrap());
        first.commit().unwrap();
        let installed = fs::canonicalize(&dst).unwrap();
        let installed_inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&installed).unwrap().ino()
        };

        for src in [installed.clone(), dst.clone()] {
            let mut txn = ReleaseTxn::allocate(&amp, "1.0.0").unwrap();
            assert!(
                stage_binary(&mut txn, &src, &dst).expect("same-file stage"),
                "same-file source {} must be staged",
                src.display()
            );
            let report = txn.commit().unwrap();
            assert!(report.failed.is_none(), "{report:?}");

            assert_eq!(fs::read(&dst).unwrap(), b"self bytes");
            assert_eq!(
                fs::read(&installed).unwrap(),
                b"self bytes",
                "the previous release's binary must be left intact"
            );
            {
                use std::os::unix::fs::MetadataExt;
                assert_eq!(fs::metadata(&installed).unwrap().ino(), installed_inode);
            }
            assert!(fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
        }
        assert_ne!(
            fs::canonicalize(&dst).unwrap(),
            installed,
            "dst must name the new release"
        );
    }
}
