//! Issue #2738: a caller's Git Trace2 settings do not follow a run.
//!
//! The Git-command tracer removed in #2738 cleared these variables as a side
//! effect of owning its own trace target. `RunCheckout::apply_to` now owns
//! them for their own reason, so this asserts the behaviour directly rather
//! than through whatever happens to clear them.
//!
//! `GIT_TRACE2_REDACT` is the one with teeth: Git redacts `Authorization:`
//! headers in Trace2 output by default, and `=0` turns that off. A caller
//! environment carrying a `GIT_TRACE2*` target together with `=0` would
//! otherwise write this run's auth headers to a sink of the caller's
//! choosing.

use super::run_checkout::{CALLER_TRACE_VARS, RunCheckout};
use std::ffi::{OsStr, OsString};
use std::process::Command;

/// `Some(None)` means the key is explicitly removed from the child; `None`
/// means the command says nothing about it and the child inherits it.
fn env_of(command: &Command, key: &str) -> Option<Option<OsString>> {
    command
        .get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v.map(OsStr::to_os_string))
}

/// The list is the one git reads, and `GIT_TRACE2_REDACT` is in it.
#[test]
fn the_cleared_list_names_every_trace2_variable_including_redact() {
    for expected in [
        "GIT_TRACE2",
        "GIT_TRACE2_CONFIG_PARAMS",
        "GIT_TRACE2_ENV_VARS",
        "GIT_TRACE2_EVENT",
        "GIT_TRACE2_PERF",
        "GIT_TRACE2_REDACT",
    ] {
        assert!(
            CALLER_TRACE_VARS.contains(&expected),
            "{expected} must be cleared from a run's environment"
        );
    }
}

/// A caller checkout is still a run: the clearing is outside the state match.
#[test]
fn a_caller_checkout_clears_the_callers_trace_settings() {
    let mut command = Command::new("runner");
    for var in CALLER_TRACE_VARS {
        command.env(var, "/caller/chosen/target");
    }
    RunCheckout::Caller(std::path::PathBuf::from("/caller")).apply_to(&mut command);
    for var in CALLER_TRACE_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed from the run's environment"
        );
    }
}

/// Nothing re-adds a trace target: a run traces nothing of its own either.
#[test]
fn no_trace_target_is_set_in_place_of_the_callers() {
    let mut command = Command::new("runner");
    RunCheckout::Caller(std::path::PathBuf::from("/caller")).apply_to(&mut command);
    for var in CALLER_TRACE_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed, never set to a target of the run's own"
        );
    }
}

// Issue #2785: release must inspect every repository that its recursive
// deletion or forced worktree removal would destroy, including the primary
// checkout. Use the same real Git checks production supplies.
// The caller holds env_lock so production Git commands see only this home
// and the real PATH, rather than another test's temporary configuration.
fn isolated_git_environment(root: &std::path::Path) -> crate::test_support::EnvGuard {
    let home = root.join("home");
    let work = root.join("work");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&work).unwrap();
    let mut env = crate::test_support::env_paths(&[
        ("HOME", Some(&home)),
        ("AMPLIHACK_WORK_ROOT", Some(&work)),
    ]);
    env.assign("PATH", crate::test_support::host_path());
    env.assign("GIT_CONFIG_GLOBAL", "/dev/null");
    env.assign("GIT_CONFIG_NOSYSTEM", "1");
    env
}

#[test]
fn checkout_release_keeps_primary_work_and_its_backing_metadata() {
    use crate::commands::cleanup::{
        git_fixture::{commit, git, make_dirty, origin_and_clone},
        git_work::folder_work,
    };
    use std::fs;
    crate::test_support::host_path();
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for state in ["clean", "dirty", "unpushed", "both"] {
        let tmp = tempfile::tempdir().unwrap();
        let _env = isolated_git_environment(tmp.path());
        let (_, caller) = origin_and_clone(&tmp.path().join("source"));
        let scratch = tmp.path().join("runtime");
        fs::create_dir(&scratch).unwrap();
        let mut checkout =
            RunCheckout::prepare_with_inputs(&caller, &scratch, false, &[], None, folder_work)
                .unwrap();
        let root = checkout.exec_dir().to_path_buf();
        let sha = if state == "unpushed" || state == "both" {
            commit(&root, "local.txt")
        } else {
            git(&root, &["rev-parse", "HEAD"])
        };
        if state == "dirty" || state == "both" {
            make_dirty(&root);
        }
        checkout
            .remove()
            .expect("intentional preservation succeeds");
        drop(checkout);
        if state == "clean" {
            assert!(!root.exists(), "clean, pushed checkout is removable");
        } else {
            assert!(root.is_dir(), "{state}: release or Drop removed local work");
            assert_eq!(git(&root, &["rev-parse", "HEAD"]), sha);
            assert!(
                !git(&root, &["show", "HEAD:base.txt"]).is_empty(),
                "backing objects must survive"
            );
            if state != "unpushed" {
                assert!(root.join("uncommitted.txt").is_file());
            }
        }
    }
}

#[test]
fn checkout_release_keeps_ignored_nested_work_and_supporting_repository() {
    use crate::commands::cleanup::{
        git_fixture::{clone_of, commit, git, origin_and_clone},
        git_work::folder_work,
    };
    use std::fs;
    crate::test_support::host_path();
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for beneath_checkout in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let _env = isolated_git_environment(tmp.path());
        let (origin, caller) = origin_and_clone(&tmp.path().join("source"));
        let scratch = tmp.path().join("runtime");
        fs::create_dir(&scratch).unwrap();
        let mut checkout =
            RunCheckout::prepare_with_inputs(&caller, &scratch, false, &[], None, folder_work)
                .unwrap();
        let root = checkout.exec_dir().to_path_buf();
        let common = std::path::PathBuf::from(git(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ));
        fs::create_dir_all(common.join("info")).unwrap();
        fs::write(common.join("info/exclude"), "nested/\n").unwrap();
        let parent = if beneath_checkout {
            root.clone()
        } else {
            root.parent().unwrap().to_path_buf()
        };
        let nested = clone_of(&origin, &parent.join("nested"));
        let sha = commit(&nested, "only-copy.txt");
        assert!(git(&root, &["status", "--porcelain"]).is_empty());

        checkout
            .remove()
            .expect("keeping work must not fail finalization");
        drop(checkout);
        assert!(
            nested.is_dir(),
            "nested repository lost, beneath_checkout={beneath_checkout}"
        );
        assert_eq!(git(&nested, &["rev-parse", "HEAD"]), sha);
    }
}

#[test]
fn storage_reclaim_preserves_real_primary_and_unregistered_nested_work() {
    use super::run_checkout::exec_storage::{self, Claim, Verdict};
    use crate::commands::cleanup::{
        git_fixture::{clone_of, commit, git, make_dirty, origin_and_clone},
        git_work::folder_work,
    };
    use std::fs;
    crate::test_support::host_path();
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for nested in [false, true] {
        for state in ["clean", "dirty", "unpushed"] {
            let tmp = tempfile::tempdir().unwrap();
            let _env = isolated_git_environment(tmp.path());
            let (origin, _) = origin_and_clone(&tmp.path().join("source"));
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let stable = tmp.path().join("identity");
            let owner = tmp.path().join("ended-run");
            fs::create_dir(&owner).unwrap();
            assert!(matches!(
                exec_storage::claim(&stable, &owner, |_| Ok(true), folder_work).unwrap(),
                Claim::Claimed(_)
            ));
            let repository = stable.join("repository.git");
            git(
                tmp.path(),
                &[
                    "clone",
                    "-q",
                    "--bare",
                    origin.to_str().unwrap(),
                    repository.to_str().unwrap(),
                ],
            );
            git(
                &repository,
                &["update-ref", "refs/remotes/origin/main", "HEAD"],
            );
            let checkout = stable.join("checkout");
            git(
                &repository,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--detach",
                    checkout.to_str().unwrap(),
                    "HEAD",
                ],
            );
            let tree = if nested {
                clone_of(&origin, &stable.join("unregistered/work"))
            } else {
                checkout.clone()
            };
            let sha = if state == "unpushed" {
                commit(&tree, "local.txt")
            } else {
                git(&tree, &["rev-parse", "HEAD"])
            };
            if state == "dirty" {
                make_dirty(&tree);
            }

            let verdict = exec_storage::reclaim(&stable, |_| Ok(true), folder_work).unwrap();
            if state == "clean" {
                assert!(matches!(verdict, Verdict::Reclaimable(_)), "{verdict:?}");
                assert!(!stable.exists());
            } else {
                let Verdict::Kept(reason) = verdict else {
                    panic!("nested={nested}, {state}: {verdict:?}");
                };
                assert!(
                    reason.contains(if state == "dirty" {
                        "uncommitted"
                    } else {
                        "unpushed"
                    }) || reason.contains("no remote branch"),
                    "{reason}"
                );
                assert_eq!(git(&tree, &["rev-parse", "HEAD"]), sha);
                if state == "dirty" {
                    assert!(tree.join("uncommitted.txt").is_file());
                }
            }
        }
    }
}

#[test]
fn tombstone_removal_keeps_work_created_before_reclamation_was_interrupted() {
    use super::run_checkout::exec_storage;
    use crate::commands::cleanup::git_fixture::{
        clone_of, commit, git, make_dirty, origin_and_clone,
    };
    use std::fs;
    crate::test_support::host_path();
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for state in ["clean", "dirty", "unpushed"] {
        let tmp = tempfile::tempdir().unwrap();
        let _env = isolated_git_environment(tmp.path());
        let (origin, _) = origin_and_clone(&tmp.path().join("source"));
        let tombstone = tmp.path().join(".reclaim-2785");
        fs::create_dir(&tombstone).unwrap();
        let tree = clone_of(&origin, &tombstone.join("work"));
        let sha = if state == "unpushed" {
            commit(&tree, "local.txt")
        } else {
            git(&tree, &["rev-parse", "HEAD"])
        };
        if state == "dirty" {
            make_dirty(&tree);
        }

        let result = exec_storage::remove_tombstone(
            &tombstone,
            None,
            crate::commands::cleanup::git_work::folder_work,
        );
        if state == "clean" {
            result.unwrap();
            assert!(!tombstone.exists());
        } else {
            assert!(
                tree.is_dir(),
                "{state}: tombstone removal destroyed work: {result:?}"
            );
            assert_eq!(git(&tree, &["rev-parse", "HEAD"]), sha);
            if state == "dirty" {
                assert!(tree.join("uncommitted.txt").is_file());
            }
        }
    }
}
