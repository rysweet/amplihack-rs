use super::fixtures::Rollback;
use super::*;

#[test]
fn rollback_rename_and_snapshot_parent_sync_failures_retain_journal_and_retry() {
    for directory in [
        "root",
        "market",
        "plugins",
        "agents",
        "home",
        "transaction",
        "published",
        "temp",
    ] {
        let fixture = Rollback::new(true, true);
        let path = match directory {
            "root" => fixture.root.clone(),
            "market" => fixture.root.join("market"),
            "plugins" => fixture.root.join("market/.agents/plugins"),
            "agents" => fixture.root.join("market/.agents"),
            "home" => fixture.home.clone(),
            "transaction" => fixture.root.parent().unwrap().to_path_buf(),
            "published" => fixture.dir.path().join("published"),
            _ => fixture.dir.path().to_path_buf(),
        };
        fixture.assert_retained_then_retry(&path);
    }
}

#[test]
fn rollback_absence_removal_sync_failures_retain_journal_and_retry() {
    for directory in ["home", "plugins", "market", "root"] {
        let fixture = Rollback::new(false, false);
        let path = match directory {
            "home" => fixture.home.clone(),
            "plugins" => fixture.root.join("market/.agents/plugins"),
            "market" => fixture.root.join("market"),
            _ => fixture.root.clone(),
        };
        fixture.assert_retained_then_retry(&path);
    }
}

#[test]
fn rollback_absent_snapshot_parents_sync_surviving_ancestors_and_retry() {
    for ancestor in ["market", "transaction"] {
        let fixture = Rollback::new(false, false);
        fs::remove_file(fixture.home.join("hooks.json")).unwrap();
        fs::remove_dir(&fixture.home).unwrap();
        fs::remove_file(fixture.root.join("market/.agents/plugins/marketplace.json")).unwrap();
        fs::remove_dir(fixture.root.join("market/.agents/plugins")).unwrap();
        fs::remove_dir(fixture.root.join("market/.agents")).unwrap();
        let surviving = if ancestor == "market" {
            fixture.root.join("market")
        } else {
            fixture.root.parent().unwrap().to_path_buf()
        };
        fixture.assert_retained_then_retry(&surviving);
        assert!(!fixture.home.exists());
        assert!(!fixture.root.join("market/.agents").exists());
    }
}

#[test]
fn rollback_post_unlink_sync_failure_reports_error_with_absent_journal() {
    let fixture = Rollback::new(true, true);
    let journal = fixture.root.join("pending.json");
    let mut injected = false;
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == fixture.root && !journal.exists() {
                injected = true;
                anyhow::bail!("injected rollback post-unlink sync failure");
            }
            Ok(())
        },
        || fixture.recover(),
    );
    assert!(injected, "rollback must synchronize journal removal");
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("injected rollback post-unlink"), "{error}");
    assert!(
        !error.contains("retained"),
        "cannot claim absent journal was retained: {error}"
    );
    assert!(!journal.exists());
    fixture.assert_original();
    fixture.recover().unwrap();
}
