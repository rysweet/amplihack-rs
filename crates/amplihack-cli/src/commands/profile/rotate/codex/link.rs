//! The default Codex home link: inspected with `lstat`, `readlink` and
//! `stat`, replaced with `symlink` and `rename`. No file is opened.

use super::{FaultPoint, account_of, fault_stop};
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

/// What the link path holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LinkState {
    /// A symlink to an account: its canonical target, label and inode.
    Link {
        target: PathBuf,
        label: String,
        ino: u64,
    },
    /// A real directory, a file, or something that cannot be examined.
    NotLink,
    /// A symlink whose target is not a direct child of the root.
    OutsideRoot,
    /// A symlink whose target does not exist.
    Dangling,
    /// Nothing at the link path.
    Missing,
}

/// The state of `link` against the profiles `root`. The target must,
/// made canonical, be a directory that is a direct child of the canonical
/// root (compared by component), named by one component not starting with
/// `.`.
pub(super) fn inspect(link: &Path, root: &Path) -> LinkState {
    match std::fs::symlink_metadata(link) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return LinkState::Missing,
        Err(_) => return LinkState::NotLink,
        Ok(m) if !m.file_type().is_symlink() => return LinkState::NotLink,
        Ok(_) => {}
    }
    let Ok(raw) = std::fs::read_link(link) else {
        return LinkState::NotLink;
    };
    let abs = if raw.is_absolute() {
        raw
    } else {
        link.parent().unwrap_or(Path::new("/")).join(raw)
    };
    if std::fs::metadata(&abs).is_err() {
        return LinkState::Dangling;
    }
    let Ok(root_c) = std::fs::canonicalize(root) else {
        return LinkState::OutsideRoot;
    };
    match account_of(&abs, &root_c) {
        Some((label, target)) => match std::fs::metadata(&target) {
            Ok(m) => LinkState::Link {
                target,
                label,
                ino: m.ino(),
            },
            Err(_) => LinkState::Dangling,
        },
        None => LinkState::OutsideRoot,
    }
}

/// `<link>.amplihack-swap`, beside the link.
pub(super) fn swap_tmp(link: &Path) -> PathBuf {
    let mut s = link.as_os_str().to_owned();
    s.push(".amplihack-swap");
    PathBuf::from(s)
}

/// Points `link` at `new_target` (absolute): a symlink at [`swap_tmp`] is
/// renamed over the link, then the link's parent is synced. A leftover
/// symlink at the temp path is replaced; anything else there is an error.
/// A `rename` error is returned with only the new temp symlink removed;
/// nothing else is deleted.
pub(super) fn relink(link: &Path, new_target: &Path, fault: Option<FaultPoint>) -> io::Result<()> {
    if !new_target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the new link target is not absolute",
        ));
    }
    let tmp = swap_tmp(link);
    match std::fs::symlink_metadata(&tmp) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(&tmp)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a symlink", tmp.display()),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    symlink(new_target, &tmp)?;
    if let Err(e) = std::fs::rename(&tmp, link) {
        // Only the symlink just made is removed.
        if let Err(rm) = std::fs::remove_file(&tmp) {
            eprintln!("warning: could not remove {}: {rm}", tmp.display());
        }
        return Err(e);
    }
    fault_stop(fault, FaultPoint::AfterRename);
    let parent = link.parent().unwrap_or(Path::new("/"));
    crate::commands::profile::auto_switch::kit::fsync_dir(parent)
}
