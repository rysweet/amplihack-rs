//! Durable pre-ledger recovery and post-ledger cleanup.
use super::*;
pub(super) fn recover_install(root: &Path, binary: &Path, home: &Path) -> Result<()> {
    path_scope::with_scope(root, home, || recover_scoped(root, binary, home))
}

fn recover_scoped(root: &Path, binary: &Path, home: &Path) -> Result<()> {
    let Some(mut pending) = regular_json(&root.join("pending.json"))? else {
        return Ok(());
    };
    ensure!(
        pending["schema_version"] == 2 && pending["codex_home"] == serde_json::to_value(home)?,
        "unsupported or out-of-scope Codex recovery journal; reconcile manually; record retained"
    );
    validate_journal(&pending, home)?;
    rollback_durability::validate_paths(root, home)?;
    preflight(&pending, root, home)?;
    if regular_json(&root.join("ownership.json"))?
        .is_some_and(|ledger| ledger["transaction"] == pending["transaction"])
    {
        preflight(&pending, root, home)?;
        storage::sync_dependencies(root, home)?;
        storage::sync_path(&root.join("ownership.json"))?;
        storage::sync_path(root)?;
        preflight(&pending, root, home)?;
        return backup_cleanup::finish(&mut pending, root, home);
    }
    rollback::restore(&pending, root, binary, home)
}

fn validate_journal(pending: &Value, home: &Path) -> Result<()> {
    ensure!(
        pending["transaction"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
            && pending["installed"].is_boolean()
            && pending["had_package"].is_boolean(),
        "invalid Codex recovery transaction; record retained"
    );
    let bytes = |value: &Value| {
        value.is_null()
            || value
                .as_array()
                .is_some_and(|a| a.iter().all(|b| b.as_u64().is_some_and(|n| n <= 255)))
    };
    ensure!(
        pending.get("config").is_some_and(bytes),
        "invalid recovery config snapshot"
    );
    if let Some(snapshots) = pending.get("snapshots") {
        ensure!(
            snapshots.is_object()
                && ["hooks", "marketplace", "ledger"]
                    .iter()
                    .all(|key| snapshots.get(*key).is_some_and(bytes)),
            "invalid recovery byte snapshots; record retained"
        );
    }
    ensure!(
        pending["target_digest"]
            .as_str()
            .is_some_and(resources::current_digest)
            && pending.get("snapshots").is_some()
            && ["config", "hooks", "marketplace", "ledger"]
                .iter()
                .all(|key| pending["expected"].get(*key).is_some_and(bytes)),
        "recovery lacks transaction ownership proof; reconcile manually; record retained"
    );
    if !pending["ledger"].is_null() {
        let previous: Ownership = serde_json::from_value(pending["ledger"].clone())?;
        ensure!(
            previous.schema_version == 1
                && previous.codex_home == home
                && resources::current_digest(&previous.package_digest),
            "legacy or invalid recovery ownership proof; reconcile manually; record retained"
        );
    }
    Ok(())
}

pub(super) fn recovery_paths(root: &Path, home: &Path) -> [(&'static str, PathBuf); 4] {
    [
        ("config", home.join("config.toml")),
        ("hooks", home.join("hooks.json")),
        (
            "marketplace",
            root.join("market/.agents/plugins/marketplace.json"),
        ),
        ("ledger", root.join("ownership.json")),
    ]
}

/// Check all resources before any destructive operation, then repeat at mutation boundaries.
pub(super) fn preflight(pending: &Value, root: &Path, home: &Path) -> Result<()> {
    path_scope::check()?;
    for (key, path) in recovery_paths(root, home) {
        let current = serde_json::to_value(snapshot(&path)?)?;
        let original = if key == "config" {
            &pending["config"]
        } else {
            &pending["snapshots"][key]
        };
        ensure!(
            (key == "config" && config::config_matches(pending, &current))
                || &current == original
                || current == pending["expected"][key],
            "foreign Codex {key} changed; reconcile manually; recovery record retained"
        );
    }
    let committed = regular_json(&root.join("ownership.json"))?
        .is_some_and(|ledger| ledger["transaction"] == pending["transaction"]);
    if pending.get("backup_cleanup").is_some() {
        ensure!(
            committed,
            "cleanup proof requires committed ledger; record retained"
        );
        ensure!(
            regular_json(&root.join("pending.json"))?.as_ref() == Some(pending),
            "backup cleanup journal changed concurrently; record retained"
        );
        backup_cleanup::validate(pending, root, home)?;
    }
    if committed {
        let ledger: Ownership = serde_json::from_value(
            regular_json(&root.join("ownership.json"))?.context("committed ledger missing")?,
        )?;
        ensure!(
            ledger.schema_version == 1
                && ledger.codex_home == home
                && Some(ledger.package_digest.as_str()) == pending["target_digest"].as_str(),
            "committed ledger scope changed; record retained"
        );
        for (key, path) in recovery_paths(root, home)
            .into_iter()
            .filter(|(key, _)| *key != "config")
        {
            ensure!(
                serde_json::to_value(snapshot(&path)?)? == pending["expected"][key],
                "committed Codex {key} changed; record retained"
            );
        }
        ensure!(
            root.join("market/plugin").is_dir()
                && Some(digest(&root.join("market/plugin"))?.as_str())
                    == pending["target_digest"].as_str(),
            "committed Codex package changed or missing; record retained"
        );
    }
    let original_digest = pending["ledger"]["package_digest"].as_str();
    for (name, backup) in [("market/plugin", false), ("previous-package", true)] {
        if backup && pending.get("backup_cleanup").is_some() {
            continue;
        }
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                ensure!(
                    meta.is_dir() && !meta.file_type().is_symlink(),
                    "nonregular Codex package; recovery record retained"
                );
                let actual = if backup {
                    backup_cleanup::original_digest(&path)?
                } else {
                    digest(&path)?
                };
                ensure!(
                    Some(actual.as_str()) == original_digest
                        || (!backup && Some(actual.as_str()) == pending["target_digest"].as_str()),
                    "foreign Codex package or backup changed; reconcile manually; recovery record retained"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if pending["had_package"] == true && !committed {
        let package = root.join("market/plugin");
        ensure!(
            root.join("previous-package").exists()
                || (package.exists() && Some(digest(&package)?.as_str()) == original_digest),
            "original Codex package backup missing; recovery record retained"
        );
    }
    Ok(())
}
