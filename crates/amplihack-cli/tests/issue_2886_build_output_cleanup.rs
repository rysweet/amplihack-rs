//! Issue #2886: a run removes its own build output when it ends.
//!
//! Drives the real `amplihack recipe run` with the stub runner of
//! `issue_2738_support`, inside that fixture's single tempfile root and
//! temporary home. No `cfg(target_os)` gate: it needs no `/proc`, so it builds
//! and passes on Linux and macOS. Nothing touches a real home, login or tmux
//! session.
//!
//! Contract:
//! - a run launched in a linked worktree with `CARGO_TARGET_DIR` and
//!   `AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR=1` leaves neither the private
//!   target folder nor the worktree's `target/` behind, and says so on stderr;
//! - a worktree with unsaved work is kept, its `target/` is still removed, and
//!   exactly one `amplihack: leaving` line names it;
//! - the run's status and stdout are unchanged by the removal.

#![cfg(unix)]

use crate::issue_2738_support::{Fixture, succeeds};
use std::fs;
use std::path::{Path, PathBuf};

const FLAG: &str = "AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR";

fn cargo_target(at: &Path) -> PathBuf {
    fs::create_dir_all(at.join("debug/.fingerprint")).unwrap();
    fs::write(
        at.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    fs::write(at.join("debug/blob"), vec![3u8; 8192]).unwrap();
    at.to_path_buf()
}

#[test]
fn a_run_with_a_private_target_folder_and_a_worktree_target_leaves_neither_behind() {
    let f = Fixture::new();
    let wt = f.add_worktree("feat-clean");
    let worktree_target = cargo_target(&wt.join("target"));
    let private = cargo_target(&f.root.join("private-target"));

    let out = Fixture::finish(
        "clean",
        f.command_in("clean", &wt)
            .env("CARGO_TARGET_DIR", &private)
            .env(FLAG, "1")
            .env_remove("AMPLIHACK_RUN_ID"),
    );

    succeeds(&out);
    assert!(!private.exists(), "the private target folder was left");
    assert!(!worktree_target.exists(), "the worktree target/ was left");
    assert!(
        wt.join(".git").is_file(),
        "the worktree itself is never removed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    for removed in [&private, &worktree_target] {
        assert!(
            stderr
                .lines()
                .any(|l| l.starts_with("amplihack: ") && l.contains(removed.to_str().unwrap())),
            "no line names {removed:?}:\n{stderr}"
        );
    }
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("warning"),
        "stdout stays the run's own"
    );
}

#[test]
fn a_target_folder_the_launcher_did_not_claim_is_left_in_place() {
    let f = Fixture::new();
    let wt = f.add_worktree("feat-unclaimed");
    let shared = cargo_target(&f.root.join("shared-target"));

    let out = Fixture::finish(
        "unclaimed",
        f.command_in("unclaimed", &wt)
            .env("CARGO_TARGET_DIR", &shared)
            .env_remove(FLAG)
            .env_remove("AMPLIHACK_RUN_ID"),
    );

    succeeds(&out);
    assert!(shared.join("debug/blob").is_file());
}

#[test]
fn a_worktree_with_unsaved_work_is_kept_its_target_is_removed_and_it_is_named_once() {
    let f = Fixture::new();
    let wt = f.add_worktree("feat-unsaved");
    fs::write(wt.join("unsaved.txt"), "not committed\n").unwrap();
    let worktree_target = cargo_target(&wt.join("target"));

    let out = Fixture::finish(
        "unsaved",
        f.command_in("unsaved", &wt)
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "1")
            .env_remove(FLAG)
            .env_remove("AMPLIHACK_RUN_ID"),
    );

    succeeds(&out);
    assert!(
        wt.join("unsaved.txt").is_file(),
        "the unsaved work survives"
    );
    assert!(!worktree_target.exists(), "its target/ is still removed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let leaving = stderr
        .lines()
        .filter(|l| l.starts_with("amplihack: leaving") && l.contains(wt.to_str().unwrap()))
        .count();
    assert_eq!(leaving, 1, "expected one 'leaving' line:\n{stderr}");
}

#[test]
fn a_nested_run_leaves_the_shared_worktree_target_alone() {
    let f = Fixture::new();
    let wt = f.add_worktree("feat-nested");
    let worktree_target = cargo_target(&wt.join("target"));

    let out = Fixture::finish(
        "nested",
        f.command_in("nested", &wt)
            .env("AMPLIHACK_RUN_ID", "parent-run-id")
            .env_remove(FLAG),
    );

    succeeds(&out);
    assert!(worktree_target.join("debug/blob").is_file());
}
