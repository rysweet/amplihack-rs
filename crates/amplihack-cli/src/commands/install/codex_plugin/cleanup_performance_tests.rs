use super::backup_cleanup_tests::committed_fixture;
use super::*;

thread_local! {
    pub(super) static DIGESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static HASHED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[test]
#[cfg(unix)]
fn cleanup_hash_work_is_bounded_independently_of_unlink_count() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for count in [8, 64] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = committed_fixture(dir.path());
        let backup = root.join("previous-package");
        for i in 0..count {
            fs::write(backup.join(format!("owned-{i}")), vec![b'a'; 4096]).unwrap();
            fs::write(
                root.join(format!("market/plugin/owned-{i}")),
                vec![b'b'; 4096],
            )
            .unwrap();
        }
        let journal = root.join("pending.json");
        let mut pending = regular_json(&journal).unwrap().unwrap();
        pending["ledger"]["package_digest"] = json!(digest(&backup).unwrap());
        pending["target_digest"] = json!(digest(&root.join("market/plugin")).unwrap());
        let mut ledger = regular_json(&root.join("ownership.json")).unwrap().unwrap();
        ledger["package_digest"] = pending["target_digest"].clone();
        pending["expected"]["ledger"] = json!(json_bytes(&ledger).unwrap());
        fs::write(root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
        fs::write(&journal, json_bytes(&pending).unwrap()).unwrap();
        DIGESTS.set(0);
        SCANS.set(0);
        HASHED_BYTES.set(0);
        recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
        assert!(!backup.exists());
        assert!(!journal.exists());
        let total_bytes = count as u64 * 8192 + 64;
        assert!(
            DIGESTS.get() <= 24,
            "{} entries: {} full digests",
            count,
            DIGESTS.get()
        );
        assert!(
            SCANS.get() <= 16,
            "{} entries: {} full scans",
            count,
            SCANS.get()
        );
        assert!(
            HASHED_BYTES.get() <= 24 * total_bytes,
            "{} entries: {} bytes repeatedly hashed",
            count,
            HASHED_BYTES.get()
        );
        println!(
            "{count} owned entries: digests={} scans={} hashed_bytes={}",
            DIGESTS.get(),
            SCANS.get(),
            HASHED_BYTES.get()
        );
    }
}

#[cfg(unix)]
#[test]
fn cleanup_rechecks_foreign_edits_after_an_actual_unlink() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for change in ["backup", "live", "hooks", "journal", "extra", "ancestor"] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = committed_fixture(dir.path());
        let backup = root.join("previous-package");
        let journal = root.join("pending.json");
        let mut changed = false;
        let result = storage::testing::with_sync_hook(
            |path| {
                if !changed && path.starts_with(&backup) && !backup.join("nested/second").exists() {
                    changed = true;
                    match change {
                        "backup" => fs::write(backup.join("resource"), b"foreign!").unwrap(),
                        "live" => {
                            fs::write(root.join("market/plugin/resource"), b"FOREIG").unwrap()
                        }
                        "hooks" => fs::write(home.join("hooks.json"), b"foreign").unwrap(),
                        "journal" => {
                            // Same-length in-place journal edit defeats length-only guards.
                            let bytes = fs::read_to_string(&journal)
                                .unwrap()
                                .replace("test", "edit");
                            fs::write(&journal, bytes).unwrap();
                        }
                        "extra" => fs::write(backup.join("nested/foreign"), b"keep").unwrap(),
                        "ancestor" => {
                            let outside = dir.path().join("outside");
                            fs::create_dir(&outside).unwrap();
                            fs::write(outside.join("foreign"), b"keep").unwrap();
                            fs::remove_dir(backup.join("nested")).unwrap();
                            std::os::unix::fs::symlink(outside, backup.join("nested")).unwrap();
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(())
            },
            || recover_install(&root, Path::new("must-not-spawn"), &home),
        );
        assert!(changed, "{change}: must mutate after an actual unlink");
        assert!(result.is_err(), "{change}: cleanup must stop");
        assert!(journal.exists(), "{change}: journal must survive");
        assert!(
            backup.join("resource").exists(),
            "{change}: subsequent backup must survive"
        );
        if change == "extra" || change == "ancestor" {
            assert_eq!(fs::read(backup.join("nested/foreign")).unwrap(), b"keep");
        }
    }
}
