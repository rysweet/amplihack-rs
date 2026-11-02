//! Cargo target folders tagged with `CACHEDIR.TAG` (issue #1808, D8).
//!
//! Backup tools that honour the Cache Directory Tagging Specification
//! (<https://bford.info/cachedir/>) skip a folder holding this tag.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The standard signature line plus a comment naming amplihack.
pub(crate) const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
# This file is a cache directory tag created by amplihack.\n\
# For information about cache directory tags see https://bford.info/cachedir/\n";

/// Length of the `Signature: <hash>` line that starts every valid tag.
const SIGNATURE_LEN: usize = 43;

/// Creates `path` (and its parents) and writes `CACHEDIR.TAG` into it. The
/// Rust twin of the tagging in `scripts/git-hook-cargo.sh`, for code that
/// creates a target folder.
///
/// Calling it again is harmless: an existing tag is left alone and nothing
/// else in the folder is touched.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn create_tagged_target_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    let tag = path.join("CACHEDIR.TAG");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o644)
        .open(&tag)
    {
        Ok(mut file) => file.write_all(CACHEDIR_TAG.as_bytes()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// True only when `path/CACHEDIR.TAG` is a regular file (not a symlink) that
/// starts with the standard signature.
pub(crate) fn is_tagged(path: &Path) -> bool {
    let tag = path.join("CACHEDIR.TAG");
    let Ok(meta) = fs::symlink_metadata(&tag) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tag)
    else {
        return false;
    };
    let signature = &CACHEDIR_TAG.as_bytes()[..SIGNATURE_LEN];
    let mut head = Vec::with_capacity(SIGNATURE_LEN);
    if file
        .take(SIGNATURE_LEN as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    head == signature
}

#[cfg(test)]
#[path = "tests_targets.rs"]
mod tests;
