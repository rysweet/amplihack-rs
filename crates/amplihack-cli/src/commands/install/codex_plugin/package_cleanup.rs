//! Retryable cleanup for an owned Codex package during uninstall.
use super::*;
use std::path::Component;

use backup_cleanup::{Entry, Kind, entry_kind, scan};

/// Persist an immutable allowlist before retiring an owned package.
pub(super) fn capture(path: &Path) -> Result<Value> {
    let entries = scan(path)?;
    ensure!(
        entries.first().is_some_and(|entry| {
            entry.path.as_os_str().is_empty() && entry.kind == Kind::Directory
        }),
        "Codex package root is missing or not a directory"
    );
    ensure!(
        scan(path)? == entries,
        "Codex package changed while recording uninstall inventory"
    );
    Ok(serde_json::to_value(entries)?)
}

fn expected_entries(inventory: &Value) -> Result<Vec<Entry>> {
    let entries: Vec<Entry> = serde_json::from_value(inventory.clone())
        .context("invalid Codex uninstall inventory; ownership record retained")?;
    let mut paths = std::collections::BTreeMap::new();
    for entry in &entries {
        ensure!(
            entry
                .path
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
                && paths.insert(entry.path.clone(), &entry.kind).is_none(),
            "unsafe or duplicate Codex uninstall inventory path"
        );
    }
    ensure!(
        paths.get(Path::new("")) == Some(&&Kind::Directory),
        "Codex uninstall inventory lacks its package root"
    );
    for path in paths.keys().filter(|path| !path.as_os_str().is_empty()) {
        ensure!(
            paths.get(path.parent().context("uninstall entry parent missing")?)
                == Some(&&Kind::Directory),
            "Codex uninstall inventory lacks a parent directory"
        );
    }
    Ok(entries)
}

/// Validate the exact surviving subset; missing entries are expected during retry.
pub(super) fn validate_survivors(path: &Path, inventory: &Value) -> Result<()> {
    let expected: std::collections::BTreeMap<_, _> = expected_entries(inventory)?
        .into_iter()
        .map(|entry| (entry.path, entry.kind))
        .collect();
    for entry in scan(path)? {
        ensure!(
            expected.get(&entry.path) == Some(&entry.kind),
            "foreign Codex package survivor changed; ownership record retained"
        );
    }
    Ok(())
}

/// Remove only authorized entries, checking each survivor immediately.
pub(super) fn remove_survivors(path: &Path, inventory: &Value) -> Result<()> {
    validate_survivors(path, inventory)?;
    let mut entries = expected_entries(inventory)?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.path.components().count()));
    for entry in entries {
        path_scope::check()?;
        for ancestor in entry.path.ancestors().skip(1) {
            let meta = match fs::symlink_metadata(path.join(ancestor)) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error.into()),
            };
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "Codex uninstall package ancestor changed; ownership record retained"
            );
        }
        let entry_path = path.join(&entry.path);
        let metadata = match fs::symlink_metadata(&entry_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        ensure!(
            entry_kind(&entry_path, &metadata)? == entry.kind,
            "Codex uninstall package survivor changed; ownership record retained"
        );
        if entry.kind == Kind::Directory {
            fs::remove_dir(&entry_path)?;
        } else {
            fs::remove_file(&entry_path)?;
        }
        storage::sync_path(
            entry_path
                .parent()
                .context("Codex cleanup entry has no parent")?,
        )?;
    }
    path_scope::check()?;
    storage::sync_path(path.parent().context("Codex package has no parent")?)?;
    Ok(())
}
