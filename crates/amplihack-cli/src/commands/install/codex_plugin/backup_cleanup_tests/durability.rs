use super::*;

#[cfg(unix)]
#[test]
fn cleanup_checks_symlink_targets_and_never_follows_them() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for substitute in [false, true] {
        cases::backup_link_case(substitute);
    }
}

#[test]
fn backup_removal_sync_failure_keeps_journal_and_can_retry() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == root && !root.join("previous-package").exists() {
                assert!(root.join("pending.json").is_file());
                anyhow::bail!("interrupted after backup removal before journal removal");
            }
            Ok(())
        },
        || recover_install(&root, Path::new("must-not-spawn"), &home),
    );
    assert!(result.is_err());
    assert!(!root.join("previous-package").exists());
    assert!(root.join("pending.json").exists());
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
    assert!(!root.join("pending.json").exists());
}

#[test]
fn proof_publication_rejects_concurrent_journal_edit_before_unlink() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let before = digest(&root.join("previous-package")).unwrap();
    let journal = root.join("pending.json");
    let mut pending = regular_json(&journal).unwrap().unwrap();
    let mut changed = pending.clone();
    changed["foreign"] = json!("concurrent edit");
    fs::write(&journal, json_bytes(&changed).unwrap()).unwrap();
    let result = backup_cleanup::finish(&mut pending, &root, &home);
    assert!(format!("{:#}", result.unwrap_err()).contains("changed concurrently"));
    assert_eq!(digest(&root.join("previous-package")).unwrap(), before);
    assert_eq!(regular_json(&journal).unwrap().unwrap(), changed);
}

#[cfg(unix)]
#[test]
fn authorized_cleanup_rejects_directory_symlink_substitution() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    interrupt_authorized_unlink(&root, &home);
    let nested = root.join("previous-package/nested");
    fs::remove_dir(&nested).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("second"), b"foreign").unwrap();
    std::os::unix::fs::symlink(&outside, &nested).unwrap();
    assert!(recover_install(&root, Path::new("must-not-spawn"), &home).is_err());
    assert_eq!(fs::read(outside.join("second")).unwrap(), b"foreign");
    assert!(root.join("pending.json").exists());
    assert!(
        fs::symlink_metadata(nested)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn unsupported_backup_fifo_is_rejected_without_opening_it() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let path = root.join("previous-package/fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let error = recover_install(&root, Path::new("must-not-spawn"), &home).unwrap_err();
    assert!(format!("{error:#}").contains("unsupported backup entry"));
    assert!(root.join("pending.json").exists());
    assert!(path.exists());
}
