use super::fixtures::Rollback;
use super::*;

#[test]
fn rollback_restored_file_sync_failures_retain_exact_journal_and_retry() {
    for key in ["config", "hooks", "marketplace", "ledger"] {
        let fixture = Rollback::new(true, true);
        let path = fixture
            .snapshots()
            .into_iter()
            .find(|(k, _)| *k == key)
            .unwrap()
            .1;
        fixture.assert_retained_then_retry(&path);
    }
}

#[test]
fn rollback_restored_package_tree_sync_failures_retain_journal_and_retry() {
    for entry in ["resource", "nested/resource", "nested", ""] {
        let fixture = Rollback::new(true, true);
        fixture.assert_retained_then_retry(&fixture.root.join("market/plugin").join(entry));
    }
}

#[test]
fn rollback_syncs_original_dependencies_before_journal_retirement() {
    let fixture = Rollback::new(true, true);
    let journal = fixture.root.join("pending.json");
    let mut trace = Vec::new();
    storage::testing::with_sync_hook(
        |path| {
            if journal.exists() {
                trace.push(path.to_path_buf());
            }
            Ok(())
        },
        || fixture.recover(),
    )
    .unwrap();
    fixture.assert_original();
    for file in fixture
        .snapshots()
        .into_iter()
        .map(|(_, path)| path)
        .chain([
            fixture.root.join("market/plugin/resource"),
            fixture.root.join("market/plugin/nested/resource"),
        ])
    {
        let content = trace
            .iter()
            .position(|p| *p == file)
            .unwrap_or_else(|| panic!("missing pre-retirement content sync: {}", file.display()));
        assert!(
            trace[content + 1..]
                .iter()
                .any(|p| Some(p.as_path()) == file.parent()),
            "missing publication sync after {}",
            file.display()
        );
    }
    for directory in [
        fixture.home.clone(),
        fixture.root.clone(),
        fixture.root.join("market"),
        fixture.root.join("market/plugin"),
        fixture.root.join("market/plugin/nested"),
        fixture.root.join("market/.agents"),
        fixture.root.join("market/.agents/plugins"),
        fixture.root.parent().unwrap().to_path_buf(),
        fixture.dir.path().join("published"),
        fixture.dir.path().to_path_buf(),
    ] {
        assert!(
            trace.contains(&directory),
            "missing pre-retirement directory sync: {}",
            directory.display()
        );
    }
    assert_eq!(
        trace
            .iter()
            .filter(|p| **p == fixture.root.join("market/plugin/nested/resource"))
            .count(),
        1,
        "sync package tree once per recovery attempt"
    );
    assert!(!journal.exists());
}

#[test]
fn rollback_revalidates_foreign_changes_after_sync_before_retiring_journal() {
    for resource in ["config", "package", "journal"] {
        let fixture = Rollback::new(true, true);
        let journal = fixture.root.join("pending.json");
        let target = match resource {
            "config" => fixture.home.join("config.toml"),
            "package" => fixture.root.join("market/plugin/resource"),
            _ => journal.clone(),
        };
        let mut changed = false;
        let result = storage::testing::with_sync_hook(
            |path| {
                if path == fixture.root && journal.exists() && !changed {
                    // This hook is reached only after the original resources have been restored.
                    fixture.assert_original();
                    changed = true;
                    if resource == "journal" {
                        let mut foreign = fixture.pending.clone();
                        foreign["transaction"] = json!("foreign transaction");
                        fs::write(&target, json_bytes(&foreign).unwrap())?;
                    } else {
                        fs::write(&target, b"foreign exact bytes")?;
                    }
                }
                Ok(())
            },
            || fixture.recover(),
        );
        assert!(changed, "rollback omitted final prerequisite barrier");
        assert!(
            result.is_err(),
            "foreign {resource} must prevent journal retirement"
        );
        assert!(journal.exists());
        if resource == "journal" {
            assert_eq!(
                regular_json(&target).unwrap().unwrap()["transaction"],
                "foreign transaction"
            );
        } else {
            assert_eq!(fs::read(target).unwrap(), b"foreign exact bytes");
        }
    }
}

#[test]
fn rollback_package_links_remain_owned_without_synchronizing_targets() {
    let mut fixture = Rollback::new(true, true);
    let backup = fixture.root.join("previous-package");
    std::os::unix::fs::symlink("resource", backup.join("relative")).unwrap();
    std::os::unix::fs::symlink("missing", backup.join("dangling")).unwrap();
    fixture.pending["ledger"]["package_digest"] = json!(digest(&backup).unwrap());
    let bytes = json_bytes(&fixture.pending["ledger"]).unwrap();
    fixture.pending["snapshots"]["ledger"] = json!(bytes);
    fixture.pending["expected"]["ledger"] = fixture.pending["snapshots"]["ledger"].clone();
    fs::write(fixture.root.join("ownership.json"), bytes).unwrap();
    fs::write(
        fixture.root.join("pending.json"),
        json_bytes(&fixture.pending).unwrap(),
    )
    .unwrap();
    let package = fixture.root.join("market/plugin");
    let mut synced = Vec::new();
    storage::testing::with_sync_hook(
        |path| {
            synced.push(path.to_path_buf());
            Ok(())
        },
        || fixture.recover(),
    )
    .unwrap();
    fixture.assert_original();
    assert!(synced.contains(&package));
    for (name, target) in [("relative", "resource"), ("dangling", "missing")] {
        assert_eq!(
            fs::read_link(package.join(name)).unwrap(),
            Path::new(target)
        );
        assert!(!synced.contains(&package.join(name)));
    }
    assert!(!fixture.root.join("pending.json").exists());
}
