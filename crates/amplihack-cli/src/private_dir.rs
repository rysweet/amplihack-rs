//! `~/.amplihack` created `0700` whatever the umask (issue #2346).
//!
//! `~/.amplihack` sits above the default work root, and the recipe runner's
//! recovery store stops any work root with a group- or world-writable folder
//! above it. Under umask `0002`, Ubuntu's default for a user with a private
//! group, a `create_dir_all` of anything under `~/.amplihack` made that
//! folder `0775`, so the first recipe run in a fresh `HOME` stopped at once.
//!
//! [`ensure_private_leaf_dir`] is called before each site that can be the
//! first to create `~/.amplihack`. It creates that one folder and nothing
//! else, so the folders below it keep the modes their sites give them.

use std::io;
use std::path::Path;

/// Creates `dir` alone, never its parents, with mode `0700` whatever the
/// umask (the umask can only remove bits).
///
/// Anything already at `dir`, a folder at any mode, a file or a symbolic
/// link, is left exactly as it was: it is not followed, inspected or changed.
/// When the parent is missing nothing is created, so `HOME` is never created
/// here. Every other error is returned.
pub(crate) fn ensure_private_leaf_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let created = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(dir)
    };
    #[cfg(not(unix))]
    let created = std::fs::DirBuilder::new().create(dir);
    match created {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "private_dir_tests.rs"]
mod tests;
