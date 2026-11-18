//! Unit tests for `git_work.rs` (issue #2785): which git trees inside a
//! folder are checked for work before the folder is removed, and the reasons
//! given when it is kept. The rules are in
//! `docs/howto/clean-up-after-recipe-runs.md`, "Which git trees are checked"
//! and "Limits on the search".
//!
//! Wire with, at the end of `git_work.rs`:
//! `#[cfg(test)] #[path = "tests_git_work.rs"] mod tests;`
//!
//! Contract:
//! - `pub(crate) fn work_inside(folder: &Path, deadline: Option<Instant>) ->
//!   Option<String>`: `None` when nothing in `folder` needs keeping, else why
//!   it is kept. It is `work_inside_with` with `WALK_LIMIT`, `TREE_LIMIT` and
//!   this process's effective uid and gid;
//! - `struct Policy { deadline: Option<Instant>, walk_limit: usize,
//!   tree_limit: usize, uid: u32, gid: u32 }` and
//!   `fn work_inside_with(folder: &Path, policy: &Policy) -> Option<String>`;
//! - `pub(crate) fn tree_work(tree: &Path) -> Option<String>`: the bare reason
//!   for one tree (`has uncommitted changes`, ...), with the same owner and
//!   mode rules for the effective uid and gid;
//! - `pub(crate) fn printable(text: &str) -> String`: C0, C1 and DEL escaped;
//! - a reason about a tree is `git tree {tree:?} {reason}`.
//!
//! Every repository is built in a temporary directory; nothing sleeps.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Instant;

// Named, not a glob, so these win over any helper of the same name in
// `git_work.rs`.
use crate::commands::cleanup::git_fixture::{
    Unlock, admin_dir_of, chmod, clean_pushed_worktree, clone_of, commit, egid, euid, git,
    linked_worktree, make_dirty, origin_and_clone, push, running_as_root,
};

const DIRTY: &str = "has uncommitted changes";
const AHEAD: &str = "has unpushed commits";
const NO_UPSTREAM: &str = "has no upstream and commits on no remote branch";
const NOT_INSPECTED: &str = "is writable by other users and was not inspected";

/// A canonical temporary root, so paths in reasons compare equal.
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

/// The defaults `work_inside` uses, with no deadline.
fn policy() -> Policy {
    Policy {
        deadline: None,
        walk_limit: WALK_LIMIT,
        tree_limit: TREE_LIMIT,
        uid: euid(),
        gid: egid(),
    }
}

fn tree_reason(tree: &Path, reason: &str) -> String {
    format!("git tree {tree:?} {reason}")
}

/// `<root>/outside/{origin.git,clone}` and the folder `<root>/folder` that a
/// cleanup path would remove.
fn outside_and_folder(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let (origin, clone) = origin_and_clone(&root.join("outside"));
    let folder = root.join("folder");
    fs::create_dir_all(&folder).unwrap();
    (origin, clone, folder)
}

// ----- trees whose repository or remotes are outside the folder --------------

#[test]
fn a_dirty_worktree_whose_repository_is_outside_keeps_the_folder() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    let wt = linked_worktree(&clone, &folder.join("tmp/wt"), "feat");
    make_dirty(&wt);

    assert_eq!(work_inside(&folder, None), Some(tree_reason(&wt, DIRTY)));
}

#[test]
fn an_unpushed_worktree_whose_repository_is_outside_keeps_the_folder() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    let wt = linked_worktree(&clone, &folder.join("tmp/wt"), "feat");
    commit(&wt, "one.txt");
    assert_eq!(
        work_inside(&folder, None),
        Some(tree_reason(&wt, NO_UPSTREAM)),
        "a branch with no upstream whose commit is on no remote branch"
    );

    push(&wt);
    commit(&wt, "two.txt");
    assert_eq!(
        work_inside(&folder, None),
        Some(tree_reason(&wt, AHEAD)),
        "a branch ahead of its upstream"
    );
}

#[test]
fn a_clean_pushed_worktree_lets_the_folder_go() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    clean_pushed_worktree(&clone, &folder.join("tmp/wt"), "feat");

    assert_eq!(work_inside(&folder, None), None);
}

#[test]
fn a_clone_of_an_outside_origin_with_an_unpushed_commit_keeps_the_folder() {
    let (_tmp, root) = scratch();
    let (origin, _, folder) = outside_and_folder(&root);
    let work = clone_of(&origin, &folder.join("tmp/work"));
    commit(&work, "local.txt");

    assert_eq!(work_inside(&folder, None), Some(tree_reason(&work, AHEAD)));
}

#[test]
fn the_first_tree_with_work_in_path_order_gives_the_reason() {
    let (_tmp, root) = scratch();
    let (origin, clone, folder) = outside_and_folder(&root);
    clean_pushed_worktree(&clone, &folder.join("a-clean"), "clean");
    let dirty = clone_of(&origin, &folder.join("b-dirty"));
    make_dirty(&dirty);
    let unpushed = clone_of(&origin, &folder.join("c-unpushed"));
    commit(&unpushed, "local.txt");

    assert_eq!(work_inside(&folder, None), Some(tree_reason(&dirty, DIRTY)));
}

// ----- trees that go with the folder ------------------------------------------

#[test]
fn a_self_contained_repository_goes_with_the_folder() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let fixture = folder.join("tmp/fixture");
    fs::create_dir_all(&fixture).unwrap();
    git(&fixture, &["init", "-q", "-b", "main"]);
    commit(&fixture, "fixture.txt");
    make_dirty(&fixture);

    assert_eq!(
        work_inside(&folder, None),
        None,
        "a `git init` with no remote is not checked, however much work it holds"
    );
}

#[test]
fn a_clone_of_an_origin_in_the_same_folder_goes_with_the_folder() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let (origin, clone) = origin_and_clone(&folder.join("tmp/fixture"));
    commit(&clone, "ahead.txt");
    make_dirty(&clone);
    assert_eq!(work_inside(&folder, None), None, "absolute path inside");

    let url = format!("file://{}", origin.display());
    git(&clone, &["remote", "set-url", "origin", &url]);
    assert_eq!(
        work_inside(&folder, None),
        None,
        "a file:// URL with no host inside the folder is inside too"
    );
}

#[test]
fn a_stray_git_folder_inside_a_dirty_repository_is_not_a_tree() {
    let (_tmp, root) = scratch();
    // The folder lies inside a dirty repository with a remote outside it.
    let outer = root.join("outer");
    fs::create_dir_all(&outer).unwrap();
    git(&outer, &["init", "-q", "-b", "main"]);
    commit(&outer, "outer.txt");
    git(
        &outer,
        &["remote", "add", "origin", "https://example.invalid/r.git"],
    );
    make_dirty(&outer);
    let folder = outer.join("inside");
    fs::create_dir_all(folder.join("sub/.git")).unwrap();

    assert_eq!(
        work_inside(&folder, None),
        None,
        "git finds the outer repository from sub, whose own .git is not a repository"
    );
}

#[test]
fn an_invalid_git_file_is_not_a_tree() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let tree = folder.join("t");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join(".git"), "this is not a gitdir line\n").unwrap();
    make_dirty(&tree);

    assert_eq!(work_inside(&folder, None), None);
}

#[test]
fn a_dangling_git_link_is_not_a_tree() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let tree = folder.join("t");
    fs::create_dir_all(&tree).unwrap();
    symlink(root.join("nowhere/.git"), tree.join(".git")).unwrap();
    make_dirty(&tree);

    assert_eq!(work_inside(&folder, None), None);
}

#[test]
fn a_symbolic_link_to_an_outside_repository_is_not_followed() {
    let (_tmp, root) = scratch();
    let (origin, _, folder) = outside_and_folder(&root);
    let outside = clone_of(&origin, &root.join("outside/dirty"));
    make_dirty(&outside);
    symlink(&outside, folder.join("link")).unwrap();

    assert_eq!(work_inside(&folder, None), None);
    assert!(outside.join("uncommitted.txt").is_file());
}

#[test]
fn a_git_link_to_an_outside_repository_makes_a_tree() {
    let (_tmp, root) = scratch();
    let (origin, _, folder) = outside_and_folder(&root);
    let outside = clone_of(&origin, &root.join("outside/linked"));
    commit(&outside, "ahead.txt");
    let tree = folder.join("t");
    fs::create_dir_all(&tree).unwrap();
    symlink(outside.join(".git"), tree.join(".git")).unwrap();
    make_dirty(&tree);

    let reason = work_inside(&folder, None).expect("a .git link is a candidate");
    assert!(
        reason.starts_with(&format!("git tree {tree:?} ")),
        "the reason names the tree that holds the link: {reason}"
    );
}

#[test]
fn a_git_folder_with_a_commondir_file_is_still_a_tree() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    let wt = linked_worktree(&clone, &folder.join("t"), "feat");
    // Turn the worktree's `.git` file into the folder it names, which points
    // at the shared repository through `commondir`.
    let admin = admin_dir_of(&wt);
    fs::remove_file(wt.join(".git")).unwrap();
    fs::rename(&admin, wt.join(".git")).unwrap();
    let common = fs::canonicalize(clone.join(".git")).unwrap();
    fs::write(wt.join(".git/commondir"), format!("{}\n", common.display())).unwrap();
    make_dirty(&wt);

    assert_eq!(work_inside(&folder, None), Some(tree_reason(&wt, DIRTY)));
}

// ----- remotes ----------------------------------------------------------------

#[test]
fn a_remote_url_that_leaves_the_folder_through_dotdot_counts_as_outside() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let c = folder.join("c");
    fs::create_dir_all(&c).unwrap();
    git(&c, &["init", "-q", "-b", "main"]);
    commit(&c, "local.txt");
    let url = format!("{}/../missing-origin", folder.display());
    git(&c, &["remote", "add", "origin", &url]);

    assert_eq!(
        work_inside(&folder, None),
        Some(tree_reason(&c, NO_UPSTREAM))
    );
}

#[test]
fn a_credential_in_a_remote_url_never_reaches_the_reason() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let c = folder.join("c");
    fs::create_dir_all(&c).unwrap();
    git(&c, &["init", "-q", "-b", "main"]);
    commit(&c, "local.txt");
    git(
        &c,
        &[
            "remote",
            "add",
            "origin",
            "https://x-access-token:SECRET@example.invalid/r.git",
        ],
    );

    let reason = work_inside(&folder, None).expect("a remote outside the folder is checked");
    assert_eq!(reason, tree_reason(&c, NO_UPSTREAM));
    for leak in ["SECRET", "x-access-token", "example.invalid"] {
        assert!(!reason.contains(leak), "{leak} leaked into {reason}");
    }
}

// ----- what git reports -------------------------------------------------------

#[test]
fn untracked_files_count_whatever_status_show_untracked_files_says() {
    let (_tmp, root) = scratch();
    let (origin, _, folder) = outside_and_folder(&root);
    let work = clone_of(&origin, &folder.join("work"));
    git(&work, &["config", "status.showUntrackedFiles", "no"]);
    make_dirty(&work);

    assert_eq!(work_inside(&folder, None), Some(tree_reason(&work, DIRTY)));
}

#[test]
fn control_characters_in_a_tree_path_are_escaped() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    let wt = linked_worktree(&clone, &folder.join("bad\u{1b}[31mname"), "feat");
    make_dirty(&wt);

    let reason = work_inside(&folder, None).expect("dirty");
    assert!(
        !reason.contains('\u{1b}'),
        "ESC reached the reason: {reason:?}"
    );
    assert!(
        !reason.chars().any(char::is_control),
        "a control character reached the reason: {reason:?}"
    );
    assert!(reason.ends_with(DIRTY), "{reason:?}");
}

#[test]
fn printable_escapes_c0_c1_and_del_and_keeps_everything_else() {
    for plain in ["plain text: a/b (c) [d]", "café ünïcödé", ""] {
        assert_eq!(printable(plain), plain);
    }
    let out = printable("a\u{1b}[31mb\u{7}c\u{9b}d\u{7f}e\nf\tg\rh\u{0}i");
    assert!(
        !out.chars().any(char::is_control),
        "C0, C1 and DEL must all be escaped: {out:?}"
    );
    assert!(out.starts_with('a') && out.ends_with('i'), "{out:?}");
    assert!(out.contains("[31mb"), "visible text is kept: {out:?}");
}

// ----- limits -----------------------------------------------------------------

#[test]
fn the_walk_limit_keeps_the_folder() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    fs::create_dir_all(folder.join("a/b")).unwrap();
    for i in 0..10 {
        fs::write(folder.join(format!("a/b/f{i}")), "x").unwrap();
    }
    let limited = Policy {
        walk_limit: 3,
        ..policy()
    };

    assert_eq!(
        work_inside_with(&folder, &limited),
        Some("search for git trees stopped after 3 entries".to_string())
    );
    assert_eq!(work_inside_with(&folder, &policy()), None);
}

#[test]
fn a_passed_deadline_keeps_the_folder() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    clean_pushed_worktree(&clone, &folder.join("tmp/wt"), "feat");
    let expected: Option<String> =
        Some("search for git trees did not finish before the end-of-run deadline".into());

    assert_eq!(work_inside(&folder, Some(Instant::now())), expected);
    let passed = Policy {
        deadline: Some(Instant::now()),
        ..policy()
    };
    assert_eq!(work_inside_with(&folder, &passed), expected);
    assert_eq!(
        work_inside(&folder, Some(Instant::now() + END_OF_RUN_DEADLINE)),
        None,
        "a deadline still ahead does not keep a clean folder"
    );
}

#[test]
fn the_tree_limit_keeps_the_folder() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    clean_pushed_worktree(&clone, &folder.join("one"), "one");
    clean_pushed_worktree(&clone, &folder.join("two"), "two");
    let limited = Policy {
        tree_limit: 1,
        ..policy()
    };

    assert_eq!(
        work_inside_with(&folder, &limited),
        Some("more than 1 git trees to check".to_string())
    );
    assert_eq!(work_inside_with(&folder, &policy()), None);
}

// ----- owner and mode rules ---------------------------------------------------

#[test]
fn a_tree_of_another_user_is_not_inspected_and_does_not_keep_the_folder() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    let wt = linked_worktree(&clone, &folder.join("tmp/wt"), "feat");
    make_dirty(&wt);
    let other = Policy {
        uid: euid().wrapping_add(1),
        ..policy()
    };

    assert_eq!(work_inside_with(&folder, &other), None);
    assert_eq!(
        work_inside_with(&folder, &policy()),
        Some(tree_reason(&wt, DIRTY)),
        "the same tree is checked for its owner"
    );
}

#[test]
fn a_git_config_writable_by_others_keeps_the_folder_uninspected() {
    let (_tmp, root) = scratch();
    let (origin, _, folder) = outside_and_folder(&root);
    let work = clone_of(&origin, &folder.join("work"));
    chmod(&work.join(".git/config"), 0o666);

    assert_eq!(
        work_inside(&folder, None),
        Some(tree_reason(&work, NOT_INSPECTED)),
        "a clean tree is still kept: git is not run where others can write"
    );
}

#[test]
fn a_group_writable_git_config_is_inspected_only_in_the_own_group() {
    let (_tmp, root) = scratch();
    let (origin, _, folder) = outside_and_folder(&root);
    let work = clone_of(&origin, &folder.join("work"));
    let config = work.join(".git/config");
    chmod(&config, 0o664);
    let group = fs::metadata(&config).unwrap().gid();
    let own = Policy {
        gid: group,
        ..policy()
    };
    let foreign = Policy {
        gid: group.wrapping_add(1),
        ..policy()
    };

    assert_eq!(
        work_inside_with(&folder, &own),
        None,
        "0664 in the own group, as umask 002 leaves it, is inspected and clean"
    );
    make_dirty(&work);
    assert_eq!(
        work_inside_with(&folder, &own),
        Some(tree_reason(&work, DIRTY))
    );
    assert_eq!(
        work_inside_with(&folder, &foreign),
        Some(tree_reason(&work, NOT_INSPECTED)),
        "group write by a group that is not the effective one is unsafe"
    );
}

#[test]
fn a_git_file_naming_a_folder_others_can_write_keeps_the_folder_uninspected() {
    let (_tmp, root) = scratch();
    let (_, clone, folder) = outside_and_folder(&root);
    let wt = clean_pushed_worktree(&clone, &folder.join("tmp/wt"), "feat");
    let admin = admin_dir_of(&wt);
    chmod(&admin, 0o777);
    let _restore = Unlock(admin);

    assert_eq!(
        work_inside(&folder, None),
        Some(tree_reason(&wt, NOT_INSPECTED))
    );
    assert_eq!(tree_work(&wt), Some(NOT_INSPECTED.to_string()));
}

#[test]
fn an_unreadable_folder_inside_is_passed_over() {
    if running_as_root() {
        return;
    }
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    let locked = folder.join("locked");
    fs::create_dir_all(locked.join("inner")).unwrap();
    chmod(&locked, 0o000);
    let _restore = Unlock(locked);

    assert_eq!(work_inside(&folder, None), None);
}

// ----- trivial folders --------------------------------------------------------

#[test]
fn a_folder_with_no_git_tree_has_nothing_to_keep() {
    let (_tmp, root) = scratch();
    let folder = root.join("folder");
    fs::create_dir_all(folder.join("tmp/plain")).unwrap();
    fs::write(folder.join("tmp/plain/file"), "x").unwrap();

    assert_eq!(work_inside(&folder, None), None);
}

#[test]
fn a_missing_folder_has_nothing_to_keep() {
    let (_tmp, root) = scratch();
    assert_eq!(work_inside(&root.join("missing"), None), None);
}

// ----- tree_work, the check for one tree --------------------------------------

#[test]
fn tree_work_gives_the_bare_reason_for_one_tree() {
    let (_tmp, root) = scratch();
    let (origin, clone, folder) = outside_and_folder(&root);

    let clean = clean_pushed_worktree(&clone, &folder.join("clean"), "clean");
    assert_eq!(tree_work(&clean), None);

    let dirty = linked_worktree(&clone, &folder.join("dirty"), "dirty");
    make_dirty(&dirty);
    assert_eq!(tree_work(&dirty), Some(DIRTY.to_string()));

    let no_upstream = linked_worktree(&clone, &folder.join("local"), "local");
    commit(&no_upstream, "local.txt");
    assert_eq!(tree_work(&no_upstream), Some(NO_UPSTREAM.to_string()));

    let ahead = clone_of(&origin, &folder.join("ahead"));
    commit(&ahead, "ahead.txt");
    assert_eq!(tree_work(&ahead), Some(AHEAD.to_string()));
}
