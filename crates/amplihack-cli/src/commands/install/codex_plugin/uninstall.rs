//! Owned native package removal.
use super::*;
pub(super) fn uninstall() -> Result<()> {
    let root = root()?;
    let home = codex_home()?;
    path_scope::with_scope(&root, &home, || uninstall_scoped(&root, &home))
}

fn uninstall_scoped(root: &Path, home: &Path) -> Result<()> {
    path_scope::check()?;
    if !root.exists() {
        return Ok(());
    }
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "refusing symlink Codex staging root"
    );
    let lock = open_package_lock(root)?;
    lock.lock_exclusive()?;
    if root.join("pending.json").exists() {
        let binary =
            selected_binary()?.context("Codex required for pending installation recovery")?;
        recover_install(root, &binary, home)?;
    }
    let ownership_path = root.join("ownership.json");
    let Some(record_json) = regular_json(&ownership_path)? else {
        return Ok(());
    };
    let mut record: Ownership = serde_json::from_value(record_json.clone())?;
    ensure!(
        record.schema_version == 1 && resources::current_digest(&record.package_digest),
        "legacy or unknown Codex ownership proof; preserve package and reconcile manually"
    );
    ensure!(
        record.codex_home == home,
        "Codex ownership home differs from captured selection; preserve record"
    );
    // Validate the on-disk package before unregistering the native plugin or
    // reconciling hooks. If an owner changed it, uninstall must preserve every
    // part of the owned installation and stop before side effects.
    let package = root.join("market/plugin");
    let retired_package = root.join("uninstall-package");
    if path_present(&package)? {
        let metadata = fs::symlink_metadata(&package)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Codex package changed to a non-directory or symlink; preserving package and ownership record"
        );
        ensure!(
            digest(&package)? == record.package_digest,
            "Codex package modified; preserving package and ownership record"
        );
    }
    if path_present(&retired_package)? {
        if !record.uninstall_pending {
            bail!(
                "uninstall package exists without an ownership transition; preserving it for manual reconciliation"
            );
        }
        let inventory = record
            .uninstall_inventory
            .as_ref()
            .context("pending uninstall lacks cleanup inventory; preserving package")?;
        package_cleanup::validate_survivors(&retired_package, inventory)?;
        if path_present(&package)? {
            package_cleanup::validate_survivors(&package, inventory)?;
        }
    } else if record.uninstall_pending && path_present(&package)? {
        let inventory = record
            .uninstall_inventory
            .as_ref()
            .context("pending uninstall lacks cleanup inventory; preserving package")?;
        package_cleanup::validate_survivors(&package, inventory)?;
    }
    let binary =
        selected_binary()?.context("Codex required to unregister owned plugin before deletion")?;
    recover_install(root, &binary, &record.codex_home)?;
    let inventory = native(&binary, &["plugin", "list", "--json"], &record.codex_home)?;
    verify_identity(&inventory, &root.join("market/plugin"))?;
    if installed(&inventory) {
        native(&binary, &["plugin", "remove", ID], &record.codex_home)?;
        ensure!(
            !installed(&native(
                &binary,
                &["plugin", "list", "--json"],
                &record.codex_home
            )?),
            "Codex plugin remains installed; retaining package"
        );
    }
    path_scope::check()?;
    reconcile_hooks(&record.codex_home, &record.hooks, &json!({}))?;
    if !record.uninstall_pending {
        record.uninstall_pending = true;
        if path_present(&package)? {
            let inventory = package_cleanup::capture(&package)?;
            ensure!(
                digest(&package)? == record.package_digest,
                "Codex package changed while recording uninstall inventory; preserving it"
            );
            record.uninstall_inventory = Some(inventory);
        }
        let updated_record = serde_json::to_value(&record)?;
        atomic_json(&ownership_path, &updated_record, Some(record_json.clone()))?;
    }
    if path_present(&package)? {
        let metadata = fs::symlink_metadata(&package)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Codex package changed to a non-directory or symlink; preserving package and ownership record"
        );
        ensure!(
            digest(&package)? == record.package_digest,
            "Codex package modified; preserving package and ownership record"
        );
        if !path_present(&retired_package)? {
            // The pending inventory authorizes recovery if a crash leaves either
            // side of this cross-directory rename visible.
            fs::rename(&package, &retired_package)?;
            storage::sync_path(
                package
                    .parent()
                    .context("Codex package has no source parent")?,
            )?;
            storage::sync_path(root)?;
        }
    }
    path_scope::check()?;
    if record.uninstall_inventory.is_some() {
        let inventory = record
            .uninstall_inventory
            .as_ref()
            .context("pending uninstall lacks cleanup inventory; preserving package")?;
        if path_present(&package)? {
            package_cleanup::remove_survivors(&package, inventory)?;
        }
        if path_present(&retired_package)? {
            package_cleanup::remove_survivors(&retired_package, inventory)?;
        }
    }
    remove_ownership_after_durable_cleanup(root, &ownership_path)?;
    // Marketplace retained: it may have acquired foreign entries.
    Ok(())
}

fn remove_ownership_after_durable_cleanup(root: &Path, ownership_path: &Path) -> Result<()> {
    // Make package removal durable before dropping the only retry authorization.
    storage::sync_path(root).context("Codex package cleanup durability barrier failed")?;
    fs::remove_file(ownership_path)?;
    storage::sync_path(root).context("Codex ownership removal durability barrier failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvGuard;

    #[test]
    fn cleanup_barrier_failure_keeps_ownership_authorization() {
        let _env_lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = home.join(".amplihack/codex");
        let codex_home = home.join(".codex");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        let ownership = root.join("ownership.json");
        fs::write(&ownership, b"pending authorization").unwrap();
        let _env = EnvGuard::set([("HOME", home.to_str().unwrap())]);

        path_scope::with_scope(&root, &codex_home, || {
            let failed = storage::testing::with_sync_hook(
                |path| {
                    if path == root {
                        anyhow::bail!("injected cleanup barrier failure");
                    }
                    Ok(())
                },
                || remove_ownership_after_durable_cleanup(&root, &ownership),
            );
            assert!(failed.is_err());
            assert!(ownership.exists());

            remove_ownership_after_durable_cleanup(&root, &ownership)?;
            assert!(!ownership.exists());
            Ok(())
        })
        .unwrap();
    }
}
