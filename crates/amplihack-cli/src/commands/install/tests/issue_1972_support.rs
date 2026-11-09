//! Fixtures shared by the `issue_1972_*` tests.
//!
//! Every expected release manifest is built from a pristine fixture tree with
//! `of_source`, so no test depends on a real release build, on the committed
//! placeholder or on an environment variable.

use super::super::bundle_manifest::of_source;
use super::super::framework_source::BinaryIdentity;
use super::super::release_manifest::ReleaseManifest;
use super::super::tag_fetch::TagFetcher;
use super::issue_1955_support::FakeFetcher;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The binary version the pure (non-install) tests use.
pub(super) const BINARY: &str = "1.2.99";

/// A bundle file every `create_source_repo` tree has.
pub(super) const LISTED: &str = "agents/marker.txt";

/// A release build of [`BINARY`].
pub(super) fn release_binary() -> BinaryIdentity {
    BinaryIdentity {
        version: BINARY.to_string(),
        may_fetch: true,
    }
}

/// The release manifest of `tree`'s `amplifier-bundle/` as it is now.
pub(super) fn release_of(tree: &Path, version: &str) -> ReleaseManifest {
    ReleaseManifest {
        version: version.to_string(),
        files: of_source(&tree.join("amplifier-bundle")).expect("hash the fixture bundle"),
    }
}

/// The release manifest a `FakeFetcher::serving(version)` tree matches, built
/// from a tree the fake itself prepares, so the two cannot drift apart.
pub(super) fn served_release(version: &str) -> ReleaseManifest {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = FakeFetcher::serving(version)
        .fetch(
            &semver::Version::parse(version).unwrap(),
            temp.path(),
            Duration::from_secs(1),
        )
        .expect("the serving fake always produces a tree");
    release_of(&root, version)
}

/// Serves `version` with one listed bundle file changed after release, and
/// records where it wrote the tree.
pub(super) fn tampered_fetcher(
    version: &str,
    written: std::rc::Rc<std::cell::RefCell<Option<PathBuf>>>,
) -> FakeFetcher {
    let version = version.to_string();
    FakeFetcher::new(move |destination| {
        let root = FakeFetcher::serving(&version).fetch(
            &semver::Version::parse(&version).unwrap(),
            destination,
            Duration::from_secs(1),
        )?;
        change_file(&root, LISTED);
        *written.borrow_mut() = Some(root.clone());
        Ok(root)
    })
}

/// Rewrite one bundle file of `tree`, so its hash no longer matches.
pub(super) fn change_file(tree: &Path, rel: &str) {
    fs::write(
        tree.join("amplifier-bundle").join(rel),
        "changed after release\n",
    )
    .unwrap();
}

/// Add a bundle file the release did not have.
pub(super) fn add_file(tree: &Path, rel: &str) {
    let path = tree.join("amplifier-bundle").join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "not in the release\n").unwrap();
}

/// Remove a bundle file the release had.
pub(super) fn remove_file(tree: &Path, rel: &str) {
    fs::remove_file(tree.join("amplifier-bundle").join(rel)).unwrap();
}

/// True when `text` has no control character other than `\n`.
pub(super) fn only_newlines_are_control(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() && c != '\n')
}

/// A file made unreadable for the life of the guard. `None` when running as
/// root, which can read it anyway, so the caller skips.
#[cfg(unix)]
pub(super) struct UnreadableFile {
    path: PathBuf,
}

#[cfg(unix)]
impl UnreadableFile {
    pub(super) fn new(path: &Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return None;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        Some(Self {
            path: path.to_path_buf(),
        })
    }
}

#[cfg(unix)]
impl Drop for UnreadableFile {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o644));
    }
}
