//! Moving artifact files and directories out of a checkout (issue #1476).
//!
//! The migration moves a multi-megabyte `graph_db` **directory** from a
//! checkout to `~/.cache`, which is routinely a different filesystem — a
//! container bind-mount, a separate `/home` partition, an NFS work tree.
//! `fs::rename` returns `EXDEV` there and std has no directory fallback, so
//! `copy_then_remove` is that fallback, exposed as its own entry point so the
//! cross-device path is testable without mounting a second filesystem.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

/// Move `src` to `dst`, falling back to copy-then-remove across devices.
///
/// Refuses a symlinked source and refuses to clobber an existing destination.
pub(crate) fn move_path(src: &Path, dst: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(src).with_context(|| format!("cannot move {}", src.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "refusing to move the symlink {}: a rename would move the link itself and every \
             later write would follow it out of the cache",
            src.display()
        );
    }
    if fs::symlink_metadata(dst).is_ok() {
        bail!(
            "refusing to overwrite the existing destination {}",
            dst.display()
        );
    }
    create_parent(dst)?;

    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => copy_then_remove(src, dst),
        Err(err) => Err(err)
            .with_context(|| format!("failed to move {} to {}", src.display(), dst.display())),
    }
}

#[cfg(unix)]
fn is_cross_device(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(libc::EXDEV)
}

#[cfg(not(unix))]
fn is_cross_device(_err: &std::io::Error) -> bool {
    false
}

/// The cross-device branch: copy, verify, then remove the source — in that
/// order, so a failure mid-copy leaves the source whole.
pub(crate) fn copy_then_remove(src: &Path, dst: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(src).with_context(|| format!("cannot copy {}", src.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("refusing to copy the symlink {}", src.display());
    }
    // Scan before copying rather than while copying: a nested symlink
    // discovered halfway through would otherwise leave the remove step free to
    // delete through it, outside the checkout.
    if metadata.file_type().is_dir() {
        reject_nested_symlinks(src)?;
    }
    create_parent(dst)?;

    if metadata.file_type().is_dir() {
        if let Err(err) = copy_dir(src, dst) {
            let _ = fs::remove_dir_all(dst);
            return Err(err);
        }
        fs::remove_dir_all(src)
            .with_context(|| format!("failed to remove {} after copying", src.display()))?;
    } else {
        if let Err(err) = copy_file(src, dst) {
            let _ = fs::remove_file(dst);
            return Err(err);
        }
        fs::remove_file(src)
            .with_context(|| format!("failed to remove {} after copying", src.display()))?;
    }
    Ok(())
}

fn create_parent(dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

fn reject_nested_symlinks(dir: &Path) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        if file_type.is_symlink() {
            bail!("refusing to copy the nested symlink {}", path.display());
        }
        if file_type.is_dir() {
            reject_nested_symlinks(&path)?;
        }
    }
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    let entries = fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", src.display()))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", from.display()))?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

/// Copy one file, restoring its modification time on the destination.
///
/// `fs::copy` stamps `SystemTime::now()`, so without this every cross-device
/// migration would make a months-old index look freshly built and the
/// staleness comparison would report stale query results with no error.
fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
    let modified = fs::symlink_metadata(src)
        .and_then(|metadata| metadata.modified())
        .with_context(|| format!("failed to read the mtime of {}", src.display()))?;
    fs::File::options()
        .write(true)
        .open(dst)
        .and_then(|file| file.set_modified(modified))
        .with_context(|| format!("failed to restore the mtime of {}", dst.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "fs_move_tests.rs"]
mod tests;
