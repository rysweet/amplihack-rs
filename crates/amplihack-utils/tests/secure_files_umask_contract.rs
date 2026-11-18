//! Issue #2346: private creation finalizes permissions, even when umask
//! removes owner access. Each mask runs in a separate child test process.
#![cfg(unix)]

use amplihack_utils::secure_files::*;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn under_masks(name: &str, body: impl FnOnce()) {
    const CHILD: &str = "AMPLIHACK_EXACT_PRIVATE_MODE_CHILD";
    if std::env::var(CHILD).as_deref()
        == Ok(std::os::unix::process::parent_id().to_string().as_str())
    {
        body();
        return;
    }
    let mut failures = Vec::new();
    for mask in [0o002, 0o777] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env(CHILD, std::process::id().to_string());
        // SAFETY: only the forked child changes its umask before exec.
        unsafe {
            command.pre_exec(move || {
                libc::umask(mask);
                Ok(())
            });
        }
        let output = command.output().unwrap();
        if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains("1 passed")
        {
            failures.push(format!(
                "umask {mask:04o}: {}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    tmp
}

// Restore outermost first so a failed creation that left a mode-0000 parent
// cannot prevent tempfile from removing this test's own paths during unwinding.
struct RestoreDirectories(Vec<PathBuf>);
impl Drop for RestoreDirectories {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        }
    }
}

#[test]
fn recursive_private_creation_restores_owner_access_on_every_new_component() {
    under_masks(
        "recursive_private_creation_restores_owner_access_on_every_new_component",
        || {
            let tmp = fixture();
            let first = tmp.path().join("state");
            let leaf = first.join("nested");
            let _restore = RestoreDirectories(vec![first.clone(), leaf.clone()]);
            create_private_directories(&leaf).unwrap();
            for path in [&first, &leaf] {
                assert_eq!(mode(path), 0o700, "{}", path.display());
            }
            fs::write(leaf.join("probe"), b"usable").unwrap();
        },
    );
}

#[test]
fn single_private_directory_creation_restores_owner_access() {
    under_masks(
        "single_private_directory_creation_restores_owner_access",
        || {
            let tmp = fixture();
            let path = tmp.path().join("state");
            let _restore = RestoreDirectories(vec![path.clone()]);
            ensure_private_dir(&path).unwrap();
            assert_eq!(mode(&path), 0o700);
            fs::write(path.join("probe"), b"usable").unwrap();
        },
    );
}

#[test]
fn new_private_file_descriptors_have_exact_owner_access_before_return() {
    under_masks(
        "new_private_file_descriptors_have_exact_owner_access_before_return",
        || {
            let tmp = fixture();
            let writers: [fn(&Path) -> std::io::Result<fs::File>; 3] = [
                open_private_write,
                open_private_append,
                open_private_nofollow,
            ];
            let mut observed = Vec::new();
            for (index, open) in writers.into_iter().enumerate() {
                let path = tmp.path().join(format!("state-{index}"));
                let mut file = open(&path).unwrap();
                observed.push(file.metadata().unwrap().mode() & 0o7777);
                file.write_all(b"private").unwrap();
            }
            assert_eq!(observed, vec![0o600; 3]);
            for index in 0..3 {
                assert_eq!(
                    fs::read(tmp.path().join(format!("state-{index}"))).unwrap(),
                    b"private"
                );
            }
        },
    );
}

#[test]
fn private_temporary_publication_is_0600_before_commit() {
    under_masks(
        "private_temporary_publication_is_0600_before_commit",
        || {
            let tmp = fixture();
            let pending = PrivateTemp::create(tmp.path(), "state.json").unwrap();
            assert_eq!(mode(pending.path()), 0o600);
            pending.commit(b"{}").unwrap();
            assert_eq!(mode(&tmp.path().join("state.json")), 0o600);
            assert_eq!(fs::read(tmp.path().join("state.json")).unwrap(), b"{}");
        },
    );
}

#[test]
fn recursive_private_creation_does_not_follow_an_existing_parent_link() {
    let tmp = fixture();
    let outside = tmp.path().join("user-managed");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    let alias = tmp.path().join("alias");
    symlink(&outside, &alias).unwrap();
    let result = create_private_directories(&alias.join("private/nested"));
    assert!(
        result.is_err(),
        "a symlink must not become a trusted parent"
    );
    assert!(!outside.join("private").exists());
    assert_eq!(mode(&outside), 0o755);
    assert!(
        fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn private_creation_preserves_existing_objects_under_restrictive_masks() {
    under_masks(
        "private_creation_preserves_existing_objects_under_restrictive_masks",
        || {
            let tmp = fixture();
            let parent = tmp.path().join("existing");
            fs::create_dir(&parent).unwrap();
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o750)).unwrap();
            create_private_directories(&parent).unwrap();
            assert_eq!(mode(&parent), 0o750);
            let path = parent.join("existing.log");
            fs::write(&path, b"old").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            open_private_write(&path)
                .unwrap()
                .write_all(b"new")
                .unwrap();
            open_private_append(&path)
                .unwrap()
                .write_all(b" appended")
                .unwrap();
            assert_eq!(mode(&path), 0o640, "only newly created files are finalized");
            assert_eq!(fs::read(&path).unwrap(), b"new appended");
            assert!(open_private_nofollow(&path).is_err());
            assert_eq!(
                mode(&path),
                0o640,
                "an admission error never repairs the file"
            );
        },
    );
}
