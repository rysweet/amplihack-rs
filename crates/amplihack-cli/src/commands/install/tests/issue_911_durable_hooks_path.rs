//! Issue #911 — a hooks path recorded in a DURABLE config file must never point
//! into a build directory.
//!
//! `find_hooks_binary()` prefers a sibling of the running executable. That is
//! correct for finding a binary to deploy and wrong for recording one: run
//! `amplihack install` from a build tree and it returns
//! `<target>/debug/amplihack-hooks`, a cargo artifact directory that gets
//! cleaned. Every hook then fails to exec.
//!
//! #911 fixed this for the Copilot plugin manifest but not for
//! `settings.json`, so Claude Code's hooks kept being wired into build trees.
//! One real config pointed at `/tmp/amplihack-precommit-target/debug/amplihack-hooks`
//! -- the shared pre-commit target dir -- and every hook broke the moment that
//! cache was reclaimed for disk space.

use super::super::binary::{choose_durable_hooks_path, looks_like_build_artifact};
use std::path::PathBuf;

#[test]
fn a_deployed_binary_beats_a_build_tree_sibling() {
    let deployed = PathBuf::from("/home/u/.local/bin/amplihack-hooks");
    let discovered = PathBuf::from("/tmp/amplihack-precommit-target/debug/amplihack-hooks");
    assert_eq!(
        choose_durable_hooks_path(Some(deployed.clone()), discovered),
        deployed,
        "a durable config must record the deployed copy, not the build tree the \
         installer happened to run from (issue #911)"
    );
}

#[test]
fn discovery_is_used_only_when_nothing_is_deployed() {
    let discovered = PathBuf::from("/usr/local/bin/amplihack-hooks");
    assert_eq!(
        choose_durable_hooks_path(None, discovered.clone()),
        discovered,
        "with no deployed copy the discovered path is all we have"
    );
}

#[test]
fn the_exact_path_that_broke_a_real_config_is_recognised_as_a_build_artifact() {
    assert!(looks_like_build_artifact(&PathBuf::from(
        "/tmp/amplihack-precommit-target/debug/amplihack-hooks"
    )));
    assert!(looks_like_build_artifact(&PathBuf::from(
        "/home/u/src/repo/target/debug/amplihack-hooks"
    )));
    assert!(looks_like_build_artifact(&PathBuf::from(
        "/home/u/src/repo/target/release/amplihack-hooks"
    )));
}

#[test]
fn an_installed_path_is_not_mistaken_for_a_build_artifact() {
    assert!(!looks_like_build_artifact(&PathBuf::from(
        "/home/u/.local/bin/amplihack-hooks"
    )));
    assert!(!looks_like_build_artifact(&PathBuf::from(
        "/home/u/.cargo/bin/amplihack-hooks"
    )));
    assert!(!looks_like_build_artifact(&PathBuf::from(
        "/usr/local/bin/amplihack-hooks"
    )));
}

#[test]
fn matching_is_on_whole_components_not_substrings() {
    // A user whose home directory is named "debug" or "target-practice" must
    // not have every install flagged. This is why the check walks components
    // instead of doing `path.contains("debug")`.
    assert!(!looks_like_build_artifact(&PathBuf::from(
        "/home/debugger/.local/bin/amplihack-hooks"
    )));
    assert!(!looks_like_build_artifact(&PathBuf::from(
        "/home/u/targeting/bin/amplihack-hooks"
    )));
}
