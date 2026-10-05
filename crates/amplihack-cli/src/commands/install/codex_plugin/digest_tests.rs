//! Distinct trees and legacy identities must never authorize deletion.
use super::backup_cleanup_tests::committed_fixture;
use super::*;

#[test]
fn framed_identity_rejects_the_exact_legacy_concatenation_collision_in_recovery() {
    for legacy in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = committed_fixture(dir.path());
        let backup = root.join("previous-package");
        fs::remove_dir_all(&backup).unwrap();
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("a"), b"").unwrap();
        fs::write(backup.join("b"), b"foreign data").unwrap();
        let original = digest(&backup).unwrap();
        // Both old trees encode exactly afilebfileforeign data.
        let old_hash = format!("{:x}", Sha256::digest(b"afilebfileforeign data"));
        let mut pending = regular_json(&root.join("pending.json")).unwrap().unwrap();
        pending["ledger"]["package_digest"] =
            json!(if legacy { old_hash } else { original.clone() });
        fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
        fs::remove_file(backup.join("b")).unwrap();
        fs::write(backup.join("a"), b"bfileforeign data").unwrap();
        assert_ne!(digest(&backup).unwrap(), original);
        let before = snapshot(&root.join("pending.json")).unwrap();
        let error = recover_install(&root, Path::new("must-not-spawn"), &home).unwrap_err();
        assert!(format!("{error:#}").contains("retained"));
        assert_eq!(fs::read(backup.join("a")).unwrap(), b"bfileforeign data");
        assert_eq!(snapshot(&root.join("pending.json")).unwrap(), before);
        assert!(
            regular_json(&root.join("pending.json"))
                .unwrap()
                .unwrap()
                .get("backup_cleanup")
                .is_none()
        );
    }
}
#[test]
fn legacy_digest_journals_retain_even_unchanged_backups_and_existing_inventories() {
    for inventory in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = committed_fixture(dir.path());
        let backup = root.join("previous-package");
        if inventory {
            // Interrupt immediately after immutable cleanup inventory publication.
            let mut published = false;
            let result = storage::testing::with_sync_hook(
                |_| {
                    let pending = regular_json(&root.join("pending.json"))?.unwrap();
                    if pending.get("backup_cleanup").is_some() {
                        published = true;
                        bail!("interrupt before unlink");
                    }
                    Ok(())
                },
                || recover_install(&root, Path::new("must-not-spawn"), &home),
            );
            assert!(result.is_err() && published);
        }
        let mut pending = regular_json(&root.join("pending.json")).unwrap().unwrap();
        let legacy = "a".repeat(64);
        pending["ledger"]["package_digest"] = json!(legacy);
        if inventory {
            pending["backup_cleanup"]["original_digest"] = json!(legacy);
        }
        fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
        let before = digest(&backup).unwrap();
        assert!(recover_install(&root, Path::new("must-not-spawn"), &home).is_err());
        assert_eq!(digest(&backup).unwrap(), before);
        assert!(root.join("pending.json").exists());
    }
}
#[test]
fn framed_digest_distinguishes_entry_boundaries_types_paths_and_link_targets() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a"), b"").unwrap();
    fs::write(root.join("b"), b"foreign data").unwrap();
    let original = digest(root).unwrap();
    assert!(resources::current_digest(&original));
    fs::remove_file(root.join("b")).unwrap();
    fs::write(root.join("a"), b"bfileforeign data").unwrap();
    assert_ne!(digest(root).unwrap(), original);
    let file = digest(root).unwrap();
    fs::remove_file(root.join("a")).unwrap();
    fs::create_dir(root.join("a")).unwrap();
    assert_ne!(digest(root).unwrap(), file);
    #[cfg(unix)]
    {
        fs::remove_dir(root.join("a")).unwrap();
        std::os::unix::fs::symlink("target-one", root.join("a")).unwrap();
        let link = digest(root).unwrap();
        fs::remove_file(root.join("a")).unwrap();
        std::os::unix::fs::symlink("target-two", root.join("a")).unwrap();
        assert_ne!(digest(root).unwrap(), link);
    }
}
