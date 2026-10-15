#![cfg(unix)]
use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

fn setup() -> (tempfile::TempDir, ProfileStore, ProfileName) {
    let d = tempfile::tempdir().unwrap();
    let s = ProfileStore::new(d.path().join("root"));
    let name = ProfileName::parse("work").unwrap();
    s.add(&name).unwrap();
    (d, s, name)
}

#[test]
fn verify_accepts_real_owned_private_directory() {
    let (_d, s, n) = setup();
    assert_eq!(s.verify_profile_dir(&n).unwrap(), s.resolve(&n).unwrap());
}

#[test]
fn verify_rejects_profile_replaced_by_symlink() {
    let (d, s, n) = setup();
    let dir = s.resolve(&n).unwrap();
    let elsewhere = d.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    symlink(&elsewhere, &dir).unwrap();
    assert!(s.verify_profile_dir(&n).is_err());
}

#[test]
fn verify_rejects_missing_directory() {
    let (_d, s, n) = setup();
    fs::remove_dir_all(s.resolve(&n).unwrap()).unwrap();
    assert!(s.verify_profile_dir(&n).is_err());
}

#[test]
fn verify_rejects_regular_file_in_place_of_directory() {
    let (_d, s, n) = setup();
    let dir = s.resolve(&n).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    fs::write(&dir, "x").unwrap();
    assert!(s.verify_profile_dir(&n).is_err());
}

#[test]
fn verify_rejects_group_or_world_writable_directory() {
    let (_d, s, n) = setup();
    let dir = s.resolve(&n).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o770)).unwrap();
    let error = s.verify_profile_dir(&n).unwrap_err();
    assert!(matches!(error, ProfileError::UnsafePermissions(_)));
    assert!(error.hint().contains("chmod go-w"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o702)).unwrap();
    assert!(s.verify_profile_dir(&n).is_err());
}

#[test]
fn verify_rejects_when_profile_root_is_a_symlink() {
    let (d, s, n) = setup();
    let real = d.path().join("root");
    let moved = d.path().join("moved");
    fs::rename(&real, &moved).unwrap();
    symlink(&moved, &real).unwrap();
    let e: Result<_, ProfileError> = s.verify_profile_dir(&n);
    assert!(e.is_err());
}
