use lbug_cache::filesystem::{InstallOutcome, install_no_replace, open_managed_root};
use std::fs;

#[test]
fn atomic_install_preserves_an_existing_winner() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("payload");
    for (attempt, bytes, expected) in [
        ("first", b"winner".as_slice(), InstallOutcome::Installed),
        ("second", b"loser".as_slice(), InstallOutcome::Existing),
    ] {
        let staging = dir.path().join(attempt);
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("archive"), bytes).unwrap();
        assert_eq!(
            install_no_replace(&staging, &destination).unwrap(),
            expected
        );
    }
    assert_eq!(fs::read(destination.join("archive")).unwrap(), b"winner");
}

#[cfg(unix)]
#[test]
fn managed_roots_reject_symlinks_and_writable_ancestors_without_repair() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("actual");
    fs::create_dir(&actual).unwrap();
    fs::set_permissions(&actual, fs::Permissions::from_mode(0o700)).unwrap();
    let link = dir.path().join("link");
    symlink(&actual, &link).unwrap();
    assert!(open_managed_root(&link).is_err());
    assert!(link.is_symlink());
    fs::set_permissions(&actual, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(open_managed_root(&actual.join("child")).is_err());
    assert!(!actual.join("child").exists());
}

#[cfg(unix)]
#[test]
fn extraction_does_not_follow_a_preexisting_directory_link() {
    use crate::support::archive;
    use lbug_cache::artifact::{ExtractionLimits, extract_archive};
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let staging = dir.path().join("staging");
    let outside = dir.path().join("outside");
    fs::create_dir(&staging).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, staging.join("include")).unwrap();
    let bytes = archive(&[("include/lbug.hpp", tar::EntryType::Regular, b"poison")]);
    assert!(extract_archive(&bytes[..], &staging, &ExtractionLimits::default()).is_err());
    assert!(!outside.join("lbug.hpp").exists());
}
