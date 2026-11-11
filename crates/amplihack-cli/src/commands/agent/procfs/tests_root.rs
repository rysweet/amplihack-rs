//! Unit tests for `procfs/root.rs` (S27, issues #2042 and #2196): which
//! values of `AMPLIHACK_TEST_PROC_ROOT` the seam honours, the reason for each
//! refusal, the per-thread override, and the shared fake table that lib
//! tests read when no override is set. `resolve(var, guard, temp_dir)` is the
//! pure part of `proc_root()`, and `root_from` maps its outcome to a root.
//! Another user's directory (`WrongOwner`) needs a second account and is not
//! built here.

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn the_proc_root_seam_needs_the_guard_a_temp_path_and_a_private_root() {
    let tmp = tempfile::tempdir().unwrap();
    let temp = tmp.path().canonicalize().unwrap();
    let inside = temp.join("proc");
    fs::create_dir(&inside).unwrap();
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o700)).unwrap();
    let g = Some(OsStr::new("1"));
    let seam = |var: &Path, guard, temp: &Path| resolve(Some(var.as_os_str()), guard, temp);
    assert_eq!(seam(&inside, g, &temp), Ok(Some(inside.clone())));
    assert_eq!(seam(&inside, None, &temp), Err(Refusal::NoGuard));
    let empty = Some(OsStr::new(""));
    assert_eq!(seam(&inside, empty, &temp), Err(Refusal::NoGuard));
    let elsewhere = temp.join("elsewhere");
    assert_eq!(seam(&inside, g, &elsewhere), Err(Refusal::OutsideTemp));
    assert_eq!(seam(&temp, g, &temp), Err(Refusal::OutsideTemp));
    assert_eq!(seam(&temp.join("missing"), g, &temp), Err(Refusal::Missing));
    assert_eq!(resolve(None, g, &temp), Ok(None));
    assert_eq!(
        resolve(empty, g, &temp),
        Ok(None),
        "an empty value is unset"
    );
    let file = temp.join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(seam(&file, g, &temp), Err(Refusal::NotADirectory));
    let escape = temp.join("link");
    std::os::unix::fs::symlink("/", &escape).unwrap();
    assert_eq!(
        seam(&escape, g, &temp),
        Err(Refusal::OutsideTemp),
        "canonical"
    );
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(seam(&inside, g, &temp), Err(Refusal::WritableByOthers));
}

// Issue #2196 (C1): the per-thread override that lib tests use. It exists
// only under `cfg(test)`, applies to the calling thread alone, runs the same
// tree checks as the variable (without the home guard), and a refused tree
// panics instead of falling back to `/proc`.

/// A 0700 tree under the temp dir that passes `check_tree`.
fn private_tree() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("amplihack-root-override-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::temp_dir())
        .unwrap();
    fs::create_dir(dir.path().join("self")).unwrap();
    dir
}

#[test]
fn check_tree_refuses_what_resolve_refuses_without_asking_for_the_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let temp = tmp.path().canonicalize().unwrap();
    let inside = temp.join("proc");
    fs::create_dir(&inside).unwrap();
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(check_tree(inside.as_os_str(), &temp), Ok(inside.clone()));
    assert_eq!(
        check_tree(temp.as_os_str(), &temp),
        Err(Refusal::OutsideTemp)
    );
    assert_eq!(
        check_tree(temp.join("missing").as_os_str(), &temp),
        Err(Refusal::Missing)
    );
    let file = temp.join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(
        check_tree(file.as_os_str(), &temp),
        Err(Refusal::NotADirectory)
    );
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        check_tree(inside.as_os_str(), &temp),
        Err(Refusal::WritableByOthers)
    );
}

#[test]
fn the_override_is_what_proc_root_returns_on_the_calling_thread() {
    let tree = private_tree();
    let canonical = tree.path().canonicalize().unwrap();
    {
        let _scope = override_proc_root(tree.path());
        assert_eq!(proc_root(), canonical);
    }
    assert_ne!(
        proc_root(),
        canonical,
        "dropping the guard must remove the override"
    );
}

#[test]
fn the_override_is_not_seen_by_another_thread() {
    let tree = private_tree();
    let canonical = tree.path().canonicalize().unwrap();
    let _scope = override_proc_root(tree.path());
    let seen = std::thread::scope(|s| s.spawn(proc_root).join().unwrap());
    assert_ne!(
        seen, canonical,
        "a spawned thread must install its own scope"
    );
    assert_eq!(
        proc_root(),
        canonical,
        "the calling thread keeps its override"
    );
}

#[test]
fn a_nested_override_restores_the_outer_one_when_it_drops() {
    let outer = private_tree();
    let inner = private_tree();
    let outer_root = outer.path().canonicalize().unwrap();
    let inner_root = inner.path().canonicalize().unwrap();
    let _outer = override_proc_root(outer.path());
    {
        let _inner = override_proc_root(inner.path());
        assert_eq!(proc_root(), inner_root);
    }
    assert_eq!(proc_root(), outer_root);
}

#[test]
fn the_override_outranks_the_variable() {
    let tree = private_tree();
    let canonical = tree.path().canonicalize().unwrap();
    let other = private_tree();
    let guard = crate::user_config::TEST_HOME_GUARD_ENV;
    crate::test_support::with_env(
        &[
            (VAR, Some(other.path().as_os_str())),
            (guard, Some(OsStr::new("1"))),
        ],
        || {
            let _scope = override_proc_root(tree.path());
            assert_eq!(proc_root(), canonical);
        },
    );
}

#[test]
#[should_panic(expected = "others can write the directory")]
fn a_tree_others_can_write_is_refused_with_a_panic() {
    let tree = private_tree();
    fs::set_permissions(tree.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let _scope = override_proc_root(tree.path());
}

#[test]
#[should_panic(expected = "override_proc_root(")]
fn a_tree_outside_the_temp_dir_is_refused_with_a_panic() {
    let _scope = override_proc_root(Path::new("/proc"));
}

// Issue #2196 (D1 step 3): with no override, a lib test reads one shared
// fake table per test process, never the host `/proc`. It is seeded like
// every fake table, private to this user, and passes `check_tree`.

use crate::test_support::proc::{SELF_ENVIRON, shared_default_root};
use std::os::unix::fs::MetadataExt;

#[test]
fn with_no_override_lib_tests_read_the_shared_default() {
    let guard = crate::user_config::TEST_HOME_GUARD_ENV;
    let root = crate::test_support::with_env(&[(VAR, None), (guard, None)], proc_root);
    assert_ne!(root, PathBuf::from("/proc"), "a lib test never reads /proc");
    assert_eq!(root, shared_default_root());
    assert_eq!(fs::read(root.join("self/environ")).unwrap(), SELF_ENVIRON);
    let meta = fs::metadata(&root).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), euid());
    assert_eq!(
        check_tree(root.as_os_str(), &std::env::temp_dir()),
        Ok(root.clone())
    );
}

#[test]
fn the_shared_default_holds_nothing_but_self() {
    let names: Vec<_> = fs::read_dir(shared_default_root())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [OsStr::new("self")], "no process entries");
}

#[test]
fn the_shared_default_is_one_directory_per_process() {
    let here = shared_default_root();
    let there = std::thread::scope(|s| s.spawn(shared_default_root).join().unwrap());
    assert_eq!(here, there);
    assert!(
        !here.starts_with("/proc"),
        "the shared default is a fake tree"
    );
}

#[test]
fn a_spawned_thread_sees_the_shared_default_not_the_override() {
    let tree = private_tree();
    let _scope = override_proc_root(tree.path());
    let seen = std::thread::scope(|s| s.spawn(proc_root).join().unwrap());
    assert_eq!(
        seen,
        shared_default_root(),
        "threads spawned by production code see the empty shared table"
    );
}

#[test]
fn every_outcome_but_an_accepted_variable_gives_the_shared_default() {
    // The step-3 branch, tested without setting the process variable.
    let tree = private_tree();
    let accepted = tree.path().canonicalize().unwrap();
    assert_eq!(root_from(Ok(Some(accepted.clone()))), accepted);
    assert_eq!(root_from(Ok(None)), shared_default_root());
    for why in [
        Refusal::NoGuard,
        Refusal::Missing,
        Refusal::OutsideTemp,
        Refusal::NotADirectory,
        Refusal::WrongOwner,
        Refusal::WritableByOthers,
    ] {
        assert_eq!(
            root_from(Err(why)),
            shared_default_root(),
            "a refused variable ({why}) must not fall back to /proc in a lib test"
        );
    }
}

#[test]
fn a_refused_variable_is_still_refused_by_resolve() {
    // `proc_root` changes under `cfg(test)`; `resolve` does not.
    let tree = private_tree();
    let temp = std::env::temp_dir();
    assert_eq!(
        resolve(Some(tree.path().as_os_str()), None, &temp),
        Err(Refusal::NoGuard)
    );
}

#[test]
fn the_fallback_warning_names_the_shared_table_in_test_builds() {
    assert!(FALLBACK.contains("shared fake table"), "{FALLBACK}");
    assert!(!FALLBACK.contains("/proc"), "{FALLBACK}");
}
