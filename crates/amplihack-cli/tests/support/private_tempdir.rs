//! Private folders for fixtures that build a `HOME` or a work root (issue
//! #2346).
//!
//! `tempfile` creates its folders with mode `0777` less the umask, so under
//! umask `0002`, Ubuntu's default for a user with a private group, a fixture
//! root is `0775`. The recipe runner's recovery store stops any work root
//! with such a folder above it ("unsafe recovery ancestor"), so a test that
//! runs a recipe under such a root fails for a reason that has nothing to do
//! with what it tests. These helpers give the mode explicitly, so the result
//! does not depend on the umask of the shell that ran `cargo test`.
//!
//! Named `private_tempdir` rather than `private_dir` so it is not confused
//! with the product module `crate::private_dir`. Uses only `std` and
//! `tempfile`. No helper changes the process umask, and none changes the
//! mode of a folder that already exists.

#![allow(dead_code)]

use std::fs::{DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

/// A drop-in for `tempfile::tempdir()` whose folder is `0700` under any
/// umask.
pub fn private_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a 0700 temporary folder")
}

/// [`private_tempdir`] under `base`, for example
/// `crate::support::scratch_base::scratch_base()`.
pub fn private_tempdir_in(base: &Path) -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir_in(base)
        .unwrap_or_else(|e| panic!("create a 0700 temporary folder in {}: {e}", base.display()))
}

/// `fs::create_dir_all` with mode `0700` on every folder it creates. Folders
/// that already exist keep their mode. Panics naming the path.
pub fn private_dir_all(path: &Path) {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap_or_else(|e| panic!("create {} with mode 0700: {e}", path.display()));
}
