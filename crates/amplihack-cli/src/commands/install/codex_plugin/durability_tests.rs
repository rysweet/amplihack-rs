//! Durability barriers and scoped fault injection.
use super::recovery_tests::interrupted;
use super::*;

// Finding 12. The implementation step supplies this PRIVATE, thread-local,
// cfg(test)-only seam. The callback runs immediately before each real sync;
// returning an error prevents that sync and must propagate to the caller.
#[test]
fn atomic_publication_checks_destination_directory_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ownership.json");
    let result = storage::testing::with_sync_hook(
        |sync_path: &Path| {
            if sync_path == dir.path() {
                anyhow::bail!("injected destination directory sync failure");
            }
            Ok(())
        },
        || atomic_json(&path, &json!({"transaction":"test"}), None),
    );
    assert!(format!("{:#}", result.unwrap_err()).contains("injected destination"));
    // Rename visibility alone is insufficient to report successful publication.
    assert!(path.is_file());
}

#[test]
fn sync_hook_reentry_panics_before_aliasing_and_resets_after_unwind() {
    let dir = tempfile::tempdir().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        storage::testing::with_sync_hook(storage::sync_path, || storage::sync_path(dir.path()))
    }));
    assert!(result.is_err(), "recursive callback must reject reentry");
    let mut calls = 0;
    storage::testing::with_sync_hook(
        |_| {
            calls += 1;
            Ok(())
        },
        || storage::sync_path(dir.path()),
    )
    .unwrap();
    assert_eq!(calls, 1, "unwind must release the scoped callback");
}

#[test]
fn committed_recovery_retains_evidence_when_dependency_sync_fails() {
    for dependency in [
        "ownership.json",
        "market/plugin/resource",
        "home/hooks.json",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home, mut pending) = interrupted(dir.path(), true);
        let ledger = json!({"schema_version":1,"codex_home":home,
            "transaction":"test","package_digest":pending["target_digest"],"hooks":{}});
        pending["expected"]["ledger"] = json!(json_bytes(&ledger).unwrap());
        fs::write(root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
        fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
        let journal_before = fs::read(root.join("pending.json")).unwrap();
        let failed_path = if dependency == "home/hooks.json" {
            home.join("hooks.json")
        } else {
            root.join(dependency)
        };
        let result = storage::testing::with_sync_hook(
            |sync_path: &Path| {
                if sync_path == failed_path {
                    anyhow::bail!("injected dependency sync failure");
                }
                Ok(())
            },
            || recover_install(&root, &dir.path().join("must-not-spawn"), &home),
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("injected dependency"));
        assert_eq!(fs::read(root.join("pending.json")).unwrap(), journal_before);
        assert_eq!(
            fs::read(root.join("previous-package/resource")).unwrap(),
            b"original"
        );
    }
}

#[test]
fn committed_recovery_syncs_dependencies_before_deleting_backup_or_journal() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home, mut pending) = interrupted(dir.path(), true);
    let ledger = json!({"schema_version":1,"codex_home":home,
        "transaction":"test","package_digest":pending["target_digest"],"hooks":{}});
    pending["expected"]["ledger"] = json!(json_bytes(&ledger).unwrap());
    fs::write(root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
    fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
    let mut synced = std::collections::HashSet::new();
    storage::testing::with_sync_hook(
        |path: &Path| {
            if root.join("previous-package").exists() {
                assert!(root.join("pending.json").is_file());
                synced.insert(path.to_path_buf());
            }
            Ok(())
        },
        || recover_install(&root, &dir.path().join("must-not-spawn"), &home),
    )
    .unwrap();
    for required in [
        root.join("ownership.json"),
        root.clone(),
        root.join("market/plugin/resource"),
        root.join("market/plugin"),
        root.join("market"),
        home.join("hooks.json"),
        home.clone(),
        root.join("market/.agents/plugins/marketplace.json"),
        root.join("market/.agents/plugins"),
    ] {
        assert!(
            synced.contains(&required),
            "missing pre-cleanup sync: {}",
            required.display()
        );
    }
    assert!(!root.join("previous-package").exists());
    assert!(!root.join("pending.json").exists());
}

#[test]
fn committed_cleanup_sync_failure_reports_removed_evidence_accurately() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home, mut pending) = interrupted(dir.path(), true);
    let ledger = json!({"schema_version":1,"codex_home":home,
        "transaction":"test","package_digest":pending["target_digest"],"hooks":{}});
    pending["expected"]["ledger"] = json!(json_bytes(&ledger).unwrap());
    fs::write(root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
    fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
    let result = storage::testing::with_sync_hook(
        |path: &Path| {
            if path == root && !root.join("pending.json").exists() {
                anyhow::bail!("injected cleanup directory sync failure");
            }
            Ok(())
        },
        || recover_install(&root, &dir.path().join("must-not-spawn"), &home),
    );
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("backup may already be removed"), "{error}");
    assert!(error.contains("injected cleanup"), "{error}");
    assert!(!root.join("previous-package").exists());
    assert!(!root.join("pending.json").exists());
    assert!(root.join("ownership.json").is_file());
}

#[cfg(unix)]
#[test]
fn durability_preserves_resource_symlinks_without_following_them() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("package");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("resource"), b"canonical").unwrap();
    std::os::unix::fs::symlink("resource", tree.join("relative")).unwrap();
    std::os::unix::fs::symlink("missing", tree.join("dangling")).unwrap();
    let mut synced = std::collections::HashSet::new();
    storage::testing::with_sync_hook(
        |path: &Path| {
            synced.insert(path.to_path_buf());
            Ok(())
        },
        || storage::sync_tree(&tree),
    )
    .unwrap();
    assert!(synced.contains(&tree));
    assert!(synced.contains(&tree.join("resource")));
    assert!(!synced.contains(&tree.join("relative")));
    assert!(!synced.contains(&tree.join("dangling")));
    assert_eq!(
        fs::read_link(tree.join("dangling")).unwrap(),
        Path::new("missing")
    );
}
