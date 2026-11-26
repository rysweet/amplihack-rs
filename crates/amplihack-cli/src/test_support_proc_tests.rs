//! Contract tests for `crate::test_support::proc` (issue #2196, C3).
//!
//! `test_support_proc.rs` declares this file as
//! `#[cfg(test)] #[path = "test_support_proc_tests.rs"] mod tests;`.
//! Every test here reads only the fake tree it creates.

use super::*;
use crate::commands::agent::procfs::proc_root;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn a_scope_points_proc_root_at_its_own_seeded_tree() {
    let scope = fake_proc_scope();
    let root = proc_root();
    assert_eq!(root, scope.path().canonicalize().unwrap());
    assert!(
        root.join("self/environ").is_file(),
        "every fake table seeds self/environ, or the scan reports Unsupported"
    );
}

#[test]
fn the_tree_is_private_to_this_user() {
    let scope = fake_proc_scope();
    let mode = fs::metadata(scope.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn dropping_the_scope_removes_the_override_before_the_tree() {
    let scope = fake_proc_scope();
    let path = scope.path().to_path_buf();
    drop(scope);
    assert!(!path.exists(), "the tree is removed with the scope");
    assert_eq!(
        proc_root(),
        shared_default_root(),
        "after the scope drops, proc_root is the shared default again"
    );
}

#[test]
fn proc_scope_at_installs_a_hand_built_tree_on_a_spawned_thread() {
    let scope = fake_proc_scope();
    let expected = scope.path().canonicalize().unwrap();
    let seen = std::thread::scope(|s| {
        s.spawn(|| {
            let _inner = proc_scope_at(&scope.fake);
            proc_root()
        })
        .join()
        .unwrap()
    });
    assert_eq!(seen, expected);
}

#[test]
fn running_session_seeds_cmdline_status_and_config_dir() {
    let scope = fake_proc_scope();
    let config = tempfile::tempdir().unwrap();
    let pid = FAKE_PID_BASE + 1;
    let entry = scope.running_session(pid, config.path());
    assert_eq!(entry.path(), scope.path().join(pid.to_string()));

    let cmdline = fs::read(entry.path().join("cmdline")).unwrap();
    assert_eq!(cmdline, b"claude\0");

    let status = fs::read_to_string(entry.path().join("status")).unwrap();
    assert!(status.contains("State:\tS"), "{status}");
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    assert!(status.contains(&format!("Uid:\t{uid}\t")), "{status}");

    let environ = fs::read(entry.path().join("environ")).unwrap();
    let want = format!("CLAUDE_CONFIG_DIR={}\0", config.path().display());
    assert_eq!(environ, want.as_bytes());

    assert!(
        fs::symlink_metadata(entry.path().join("exe")).is_err(),
        "running_session writes no exe link"
    );
}

#[test]
fn a_removed_session_is_gone_from_the_table() {
    let scope = fake_proc_scope();
    let config = tempfile::tempdir().unwrap();
    let pid = FAKE_PID_BASE + 2;
    scope.running_session(pid, config.path());
    scope.fake.remove(pid);
    assert!(!proc_root().join(pid.to_string()).exists());
}

#[test]
#[should_panic]
fn running_session_refuses_a_pid_below_the_fake_floor() {
    let scope = fake_proc_scope();
    let config = tempfile::tempdir().unwrap();
    scope.running_session(std::process::id(), config.path());
}

#[test]
fn proc_scope_at_nests_inside_a_scope_and_restores_it() {
    let outer = fake_proc_scope();
    let inner = FakeProc::new();
    let outer_root = outer.path().canonicalize().unwrap();
    {
        let _inner = proc_scope_at(&inner);
        assert_eq!(proc_root(), inner.path().canonicalize().unwrap());
    }
    assert_eq!(proc_root(), outer_root);
}

#[test]
fn the_shared_default_is_seeded_like_every_fake_table() {
    let root = shared_default_root();
    assert_eq!(fs::read(root.join("self/environ")).unwrap(), SELF_ENVIRON);
    let mode = fs::metadata(root).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}
