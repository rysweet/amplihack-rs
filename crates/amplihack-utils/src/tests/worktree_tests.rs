//! Runtime resolution must not initialise or mutate non-repositories.
use super::*;
use std::fs;

#[test]
fn non_repository_is_rejected_without_creating_state() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source.txt"), "source\n").unwrap();
    assert!(get_shared_runtime_dir(root.path()).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(
        fs::read(root.path().join("source.txt")).unwrap(),
        b"source\n"
    );
}

#[test]
fn nested_checkout_does_not_authorise_parent_workspace() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(
        amplihack_git::command_in(&nested)
            .args(["init", "--quiet", "--template="])
            .status()
            .unwrap()
            .success()
    );
    assert!(get_shared_runtime_dir(root.path()).is_err());
    assert!(!root.path().join(".git").exists());
    assert!(!root.path().join(".claude").exists());
}

#[test]
fn physical_resolution_follows_existing_ancestors_without_creating_anything() {
    let root = tempfile::tempdir().unwrap();
    let actual = root.path().canonicalize().unwrap();
    let path = amplihack_git::launcher_state::physical(&actual.join("missing/state")).unwrap();
    assert_eq!(path, actual.join("missing/state"));
    assert!(!path.exists());
    assert!(amplihack_git::launcher_state::physical(Path::new("relative/state")).is_err());
    assert!(amplihack_git::launcher_state::physical(&actual.join("../escape")).is_err());
}
