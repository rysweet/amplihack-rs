//! Owned native package removal.
use super::*;
pub(super) fn uninstall() -> Result<()> {
    let root = root()?;
    if !root.exists() {
        return Ok(());
    }
    ensure!(
        !fs::symlink_metadata(&root)?.file_type().is_symlink(),
        "refusing symlink Codex staging root"
    );
    let lock = fs::File::open(&root)?;
    lock.lock_exclusive()?;
    if root.join("pending.json").exists() {
        let binary =
            selected_binary()?.context("Codex required for pending installation recovery")?;
        recover_install(&root, &binary, &codex_home()?)?;
    }
    let Some(record) = regular_json(&root.join("ownership.json"))? else {
        return Ok(());
    };
    let record: Ownership = serde_json::from_value(record)?;
    ensure!(
        record.schema_version == 1,
        "unknown Codex ownership version; refusing removal"
    );
    let binary =
        selected_binary()?.context("Codex required to unregister owned plugin before deletion")?;
    recover_install(&root, &binary, &record.codex_home)?;
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
    reconcile_hooks(&record.codex_home, &record.hooks, &json!({}))?;
    let package = root.join("market/plugin");
    if package.exists() && digest(&package)? == record.package_digest {
        fs::remove_dir_all(package)?;
    } else if package.exists() {
        bail!("Codex package modified; preserving package and ownership record");
    }
    fs::remove_file(root.join("ownership.json"))?;
    // Marketplace retained: it may have acquired foreign entries.
    Ok(())
}
