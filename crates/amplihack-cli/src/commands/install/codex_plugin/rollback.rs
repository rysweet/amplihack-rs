//! Guarded rollback: native writes finish before exact original config restoration.
use super::*;

pub(super) fn restore(pending: &Value, root: &Path, binary: &Path, home: &Path) -> Result<()> {
    let journal = snapshot(&root.join("pending.json"))?.context("rollback journal missing")?;
    ensure!(
        serde_json::from_slice::<Value>(&journal)? == *pending,
        "Codex rollback journal changed; record retained"
    );
    guard(pending, root, home, &journal)?;
    let package = root.join("market/plugin");
    let inventory = native(binary, &["plugin", "list", "--json"], home)?;
    verify_identity(&inventory, &package)?;
    if pending["installed"] == false && installed(&inventory) {
        guard(pending, root, home, &journal)?;
        native(binary, &["plugin", "remove", ID], home)?;
        guard(pending, root, home, &journal)?;
    }
    let backup = root.join("previous-package");
    guard(pending, root, home, &journal)?;
    if backup.exists() {
        ensure!(
            pending["ledger"]["package_digest"].as_str() == Some(digest(&backup)?.as_str()),
            "previous Codex package changed; recovery retained for manual repair"
        );
        if package.exists() {
            guard(pending, root, home, &journal)?;
            fs::remove_dir_all(&package)?;
        }
        guard(pending, root, home, &journal)?;
        fs::rename(&backup, &package)?;
    } else if pending["had_package"] == false && package.exists() {
        fs::remove_dir_all(&package)?;
    }
    for (key, path) in recovery::recovery_paths(root, home)
        .into_iter()
        .filter(|(key, _)| *key != "config")
    {
        guard(pending, root, home, &journal)?;
        restore_bytes(&path, &pending["snapshots"][key], &pending["expected"][key])?;
    }
    if pending["installed"] == true {
        guard(pending, root, home, &journal)?;
        native(binary, &["plugin", "add", ID], home)?;
        guard(pending, root, home, &journal)?;
        let inventory = native(binary, &["plugin", "list", "--json"], home)?;
        verify_identity(&inventory, &package)?;
        ensure!(
            installed(&inventory),
            "restored Codex registration missing; record retained"
        );
    }
    guard(pending, root, home, &journal)?;
    let config = home.join("config.toml");
    let current = serde_json::to_value(snapshot(&config)?)?;
    ensure!(
        config::config_matches(pending, &current),
        "foreign Codex config changed; recovery record retained"
    );
    restore_bytes(&config, &pending["config"], &current)?;
    verify_original(pending, root, home, &journal)?;
    rollback_durability::sync_restored(pending, root, home)
        .context("Codex rollback prerequisite synchronization failed; journal retained")?;
    verify_original(pending, root, home, &journal)?;
    fs::remove_file(root.join("pending.json"))?;
    storage::sync_path(root)
        .context("Codex rollback journal removed; removal synchronization failed")
}

fn guard(pending: &Value, root: &Path, home: &Path, journal: &[u8]) -> Result<()> {
    rollback_durability::validate_paths(root, home)?;
    ensure!(
        snapshot(&root.join("pending.json"))?.as_deref() == Some(journal),
        "Codex rollback journal changed concurrently; record retained"
    );
    preflight(pending, root, home)
}

fn verify_original(pending: &Value, root: &Path, home: &Path, journal: &[u8]) -> Result<()> {
    guard(pending, root, home, journal)?;
    for (key, path) in recovery::recovery_paths(root, home) {
        let original = if key == "config" {
            &pending["config"]
        } else {
            &pending["snapshots"][key]
        };
        ensure!(
            serde_json::to_value(snapshot(&path)?)? == *original,
            "Codex rollback {key} differs from original; record retained"
        );
    }
    ensure!(
        fs::symlink_metadata(root.join("previous-package"))
            .err()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "Codex rollback backup remains; record retained"
    );
    let package = root.join("market/plugin");
    if pending["had_package"] == true {
        ensure!(
            Some(digest(&package)?.as_str()) == pending["ledger"]["package_digest"].as_str(),
            "original Codex package not restored; record retained"
        );
    } else {
        ensure!(
            fs::symlink_metadata(package)
                .err()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "Codex rollback package should be absent; record retained"
        );
    }
    Ok(())
}
