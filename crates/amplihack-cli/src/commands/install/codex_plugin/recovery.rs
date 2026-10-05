//! Durable pre-ledger recovery and post-ledger cleanup.
use super::*;
pub(super) fn restore_json(path: &Path, value: &Value) -> Result<()> {
    let current = regular_json(path)?;
    if value.is_null() {
        if current.is_some() {
            fs::remove_file(path)?;
        }
    } else {
        atomic_json(path, value, current)?;
    }
    Ok(())
}
pub(super) fn recover_install(root: &Path, binary: &Path, home: &Path) -> Result<()> {
    let Some(pending) = regular_json(&root.join("pending.json"))? else {
        return Ok(());
    };
    ensure!(
        pending["schema_version"] == 1 && pending["codex_home"] == serde_json::to_value(home)?,
        "pending Codex recovery scope changed; restore the original CODEX_HOME before retrying"
    );
    validate_journal(&pending, home)?;
    let package = root.join("market/plugin");
    if regular_json(&root.join("ownership.json"))?
        .is_some_and(|ledger| ledger["transaction"] == pending["transaction"])
    {
        if root.join("previous-package").exists() {
            fs::remove_dir_all(root.join("previous-package"))?;
        }
        fs::remove_file(root.join("pending.json"))?;
        return Ok(());
    }
    let inventory = native(binary, &["plugin", "list", "--json"], home)?;
    verify_identity(&inventory, &package)?;
    if pending["installed"] == false && installed(&inventory) {
        native(binary, &["plugin", "remove", ID], home)?;
    }
    let backup = root.join("previous-package");
    if backup.exists() {
        ensure!(
            pending["ledger"]["package_digest"].as_str() == Some(digest(&backup)?.as_str()),
            "previous Codex package changed; recovery retained for manual repair"
        );
        if package.exists() {
            fs::remove_dir_all(&package)?;
        }
        fs::rename(&backup, &package)?;
    } else if pending["had_package"] == false && package.exists() {
        fs::remove_dir_all(&package)?;
    }
    let config = home.join("config.toml");
    if let Some(bytes) = pending["config"].as_array() {
        let bytes: Vec<u8> = bytes
            .iter()
            .map(|v| {
                v.as_u64()
                    .context("invalid recovery config byte")
                    .and_then(|v| u8::try_from(v).context("invalid config byte"))
            })
            .collect::<Result<_>>()?;
        let mut staged = tempfile::NamedTempFile::new_in(home)?;
        if config.exists() {
            staged
                .as_file()
                .set_permissions(fs::metadata(&config)?.permissions())?;
        }
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(&config).context("config recovery failed")?;
    } else if config.exists() {
        fs::remove_file(&config)?;
    }
    if let Some(snapshots) = pending.get("snapshots") {
        restore_bytes(&home.join("hooks.json"), &snapshots["hooks"])?;
        restore_bytes(
            &root.join("market/.agents/plugins/marketplace.json"),
            &snapshots["marketplace"],
        )?;
        restore_bytes(&root.join("ownership.json"), &snapshots["ledger"])?;
    } else {
        restore_json(&home.join("hooks.json"), &pending["hooks"])?;
        restore_json(
            &root.join("market/.agents/plugins/marketplace.json"),
            &pending["marketplace"],
        )?;
        restore_json(&root.join("ownership.json"), &pending["ledger"])?;
    }
    if pending["installed"] == true {
        native(binary, &["plugin", "add", ID], home)?;
    }
    fs::remove_file(root.join("pending.json"))?;
    Ok(())
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
    if !pending["ledger"].is_null() {
        let previous: Ownership = serde_json::from_value(pending["ledger"].clone())?;
        ensure!(
            previous.schema_version == 1 && previous.codex_home == home,
            "invalid recovery ownership scope; record retained"
        );
    }
    Ok(())
}
