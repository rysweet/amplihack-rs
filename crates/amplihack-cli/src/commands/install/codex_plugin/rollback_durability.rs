//! Rollback prerequisites, including absence and ancestor entry publication.
use super::*;
use std::collections::BTreeSet;

/// Reject symlink or non-directory components before restoration or synchronization.
/// Missing parents are legitimate for original absence and partially completed retries.
pub(super) fn validate_paths(root: &Path, home: &Path) -> Result<()> {
    for directory in [root.to_path_buf(), home.to_path_buf(), root.join("market")]
        .into_iter()
        .chain(
            recovery::recovery_paths(root, home)
                .into_iter()
                .filter_map(|(_, path)| path.parent().map(Path::to_path_buf)),
        )
    {
        existing_ancestors(&directory)?;
    }
    Ok(())
}

/// Synchronize original contents, then containing entries from deepest directory
/// to captured caller anchors. Repeat on retry even if this attempt mutated nothing.
pub(super) fn sync_restored(pending: &Value, root: &Path, home: &Path) -> Result<()> {
    validate_paths(root, home)?;
    let mut directories = BTreeSet::new();
    let package = root.join("market/plugin");
    if pending["had_package"] == true {
        // Resource links are entries only; sync_tree never follows their targets.
        storage::sync_tree(&package)?;
    }
    for (_, path) in recovery::recovery_paths(root, home) {
        if snapshot(&path)?.is_some() {
            storage::sync_path(&path)?;
        }
        directories.extend(existing_ancestors(
            path.parent().context("rollback snapshot parent missing")?,
        )?);
    }
    // Destination and source parents of the package rename/removal, even on retry.
    directories.extend(existing_ancestors(&root.join("market"))?);
    directories.extend(existing_ancestors(root)?);
    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in directories {
        // Recheck ancestors at the barrier, not only at initial validation.
        path_scope::check()?;
        storage::sync_path(&path)?;
    }
    Ok(())
}

fn existing_ancestors(path: &Path) -> Result<Vec<PathBuf>> {
    path_scope::ancestors(path)
}
