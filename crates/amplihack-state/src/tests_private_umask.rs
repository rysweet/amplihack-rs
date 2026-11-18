//! Exact private modes are restored before files are returned or published.
//! Existing 0002 tests cover permissive masks; these remove all owner bits.
use crate::permission_tests::{mode, under_umask};
use crate::{AtomicCounter, AtomicFlag, AtomicJsonFile, FileLock};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

#[test]
fn private_flags_restore_owner_access_before_reporting_creation() {
    under_umask(
        "tests_private_umask::private_flags_restore_owner_access_before_reporting_creation",
        0o777,
        || {
            let dir = fixture();
            let flag = AtomicFlag::new_private(dir.path().join("ready"));
            assert!(flag.set().unwrap());
            assert_eq!(mode(flag.path()), 0o600);
            assert!(!flag.set().unwrap());
            flag.clear().unwrap();
        },
    );
}

#[test]
fn private_lock_restores_owner_access_and_can_be_reopened() {
    under_umask(
        "tests_private_umask::private_lock_restores_owner_access_and_can_be_reopened",
        0o777,
        || {
            let dir = fixture();
            let path = dir.path().join("state.lock");
            let lock = FileLock::exclusive_private(&path, Duration::ZERO).unwrap();
            assert_eq!(mode(&path), 0o600);
            drop(lock);
            FileLock::exclusive_private(&path, Duration::ZERO).unwrap();
        },
    );
}

#[test]
fn private_json_finalizes_pending_permissions_before_publication() {
    under_umask(
        "tests_private_umask::private_json_finalizes_pending_permissions_before_publication",
        0o777,
        || {
            let dir = fixture();
            let path = dir.path().join("state.json");
            let lock_path = dir.path().join("state.json.lock");
            // Isolate publication from lock creation, which has its own test.
            fs::write(&lock_path, b"").unwrap();
            fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
            let file = AtomicJsonFile::new_private(&path);
            // Serialization runs after the temporary file is opened and before
            // publication. Observe its permissions at that boundary, so chmod
            // after rename cannot satisfy this contract.
            struct InspectPending<'a>(&'a Path);
            impl serde::Serialize for InspectPending<'_> {
                fn serialize<S: serde::Serializer>(
                    &self,
                    serializer: S,
                ) -> Result<S::Ok, S::Error> {
                    let entries: Vec<_> = fs::read_dir(self.0)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                        .filter(|path| path.file_name().unwrap() != "state.json.lock")
                        .collect();
                    let modes: Vec<_> = entries.iter().map(|path| mode(path)).collect();
                    if modes != [0o600] || self.0.join("state.json").exists() {
                        return Err(serde::ser::Error::custom(format!(
                            "expected one unpublished 0600 temporary file, observed modes {modes:?}"
                        )));
                    }
                    serde_json::json!({"count": 1}).serialize(serializer)
                }
            }
            file.write(&InspectPending(dir.path())).unwrap();
            assert_eq!(mode(&path), 0o600);
            file.update::<serde_json::Value, _>(|value| {
                value["count"] = 2.into();
            })
            .unwrap();
            assert_eq!(
                file.read::<serde_json::Value>().unwrap().unwrap()["count"],
                2
            );
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&lock_path), 0o600);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        },
    );
}

#[test]
fn private_counter_restores_owner_access_for_repeated_updates() {
    under_umask(
        "tests_private_umask::private_counter_restores_owner_access_for_repeated_updates",
        0o777,
        || {
            let dir = fixture();
            let path = dir.path().join("sequence.json");
            let counter = AtomicCounter::new_private(&path);
            assert_eq!(counter.increment().unwrap(), 1);
            assert_eq!(counter.increment().unwrap(), 2);
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&path.with_extension("json.lock")), 0o600);
        },
    );
}
