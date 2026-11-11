//! Issue #1955 D1: what startup self-heal does when the release tag can't be
//! fetched. Only a failed fetch with a compatible installed bundle may continue,
//! with a warning, and the unchanged stamp makes the next launch try again.

use super::super::tag_fetch::{FetchedWrongVersion, SourceFetchFailed};
use super::super::version_stamp;
use super::helpers::create_minimal_staged_assets;
use super::issue_1526_implicit_downgrade::{IsolatedHome, args, newer_version};
use super::issue_1955_support::older_than_binary;
use crate::self_heal::ensure_assets_match_binary_version_with;
use std::cell::Cell;
use std::fs;
use std::path::Path;

fn launch_args() -> Vec<std::ffi::OsString> {
    args(&["amplihack", "launch"])
}

fn fetch_failed(reason: &str) -> anyhow::Error {
    anyhow::Error::new(SourceFetchFailed::Failed {
        version: crate::VERSION.to_string(),
        reason: reason.to_string(),
    })
}

fn not_allowed() -> anyhow::Error {
    anyhow::Error::new(SourceFetchFailed::NotAllowed {
        version: crate::VERSION.to_string(),
    })
}

fn stale_install(home: &Path) -> String {
    create_minimal_staged_assets(home);
    let stamp = older_than_binary();
    version_stamp::write_installed_version(&stamp).unwrap();
    stamp
}

fn write_stale_bundle(home: &Path) {
    fs::write(
        home.join(".amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml"),
        "name: \"smart-orchestrator\"\nsteps:\n  - id: \"parse\"\n    type: \"shell\"\n    \
         command: |\n      HELPER=\"$(amplihack resolve-bundle-asset helper-path)\"\n",
    )
    .unwrap();
}

/// Runs self-heal with `error` from the installer. Returns (result, notice).
fn run_with_failure(error: anyhow::Error) -> (anyhow::Result<()>, String) {
    let mut notice = Vec::new();
    let result =
        ensure_assets_match_binary_version_with(&launch_args(), &mut notice, || Err(error));
    (result, String::from_utf8(notice).unwrap())
}

#[test]
fn a_failed_fetch_with_a_compatible_bundle_warns_and_runs_the_command() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    let stamp = stale_install(temp.path());

    let (result, notice) = run_with_failure(fetch_failed("network request timed out"));

    result.expect("the command must still run");
    assert_eq!(
        notice,
        format!(
            "amplihack: warning: could not fetch the framework bundle for v{} (network request \
             timed out); keeping the installed bundle. A later launch will retry.\n",
            crate::VERSION
        )
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(stamp.as_str()),
        "the old stamp is what makes the next launch retry"
    );
}

#[test]
fn a_build_that_may_not_fetch_gets_its_own_warning_with_no_promise_of_a_retry() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());

    let (result, notice) = run_with_failure(not_allowed());

    result.expect("the command must still run");
    assert_eq!(
        notice,
        format!(
            "amplihack: warning: this build ({}) cannot fetch a release tag, and no local source \
             matches it; keeping the installed bundle. Run `amplihack install --local <PATH>` to \
             replace it.\n",
            crate::VERSION
        )
    );
}

#[test]
fn the_warning_is_one_line_with_no_control_characters() {
    // SEC-14: the reason can hold text a remote or a hostile tree controlled.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());

    let (result, notice) = run_with_failure(fetch_failed("line one\nline two\u{1b}[2J\r"));

    result.expect("the command must still run");
    assert_eq!(notice.lines().count(), 1, "{notice:?}");
    assert!(notice.contains("could not fetch"), "{notice:?}");
    assert!(
        !notice.trim_end_matches('\n').chars().any(char::is_control),
        "{notice:?}"
    );
}

#[test]
fn the_next_launch_tries_again() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());
    let attempts = Cell::new(0u32);

    for _ in 0..2 {
        let mut notice = Vec::new();
        ensure_assets_match_binary_version_with(&launch_args(), &mut notice, || {
            attempts.set(attempts.get() + 1);
            Err(fetch_failed("offline"))
        })
        .expect("the command must still run");
    }

    assert_eq!(attempts.get(), 2);
}

#[test]
fn the_error_is_recognised_through_added_context() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());

    let (result, _) = run_with_failure(fetch_failed("offline").context("staging failed"));

    result.expect("context added on the way up must not hide the failed fetch");
}

#[test]
fn a_failed_fetch_with_no_bundle_is_still_a_hard_failure() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    version_stamp::write_installed_version(&older_than_binary()).unwrap();

    let (result, notice) = run_with_failure(fetch_failed("offline"));

    assert!(result.is_err(), "nothing usable is installed");
    assert!(!notice.contains("keeping the installed bundle"), "{notice}");
}

#[test]
fn a_failed_fetch_with_an_incompatible_bundle_is_still_a_hard_failure() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());
    write_stale_bundle(temp.path());

    let (result, notice) = run_with_failure(fetch_failed("offline"));

    assert!(
        result.is_err(),
        "this bundle can't run this binary's recipes"
    );
    assert!(!notice.contains("keeping the installed bundle"), "{notice}");
}

#[test]
fn a_build_that_may_not_fetch_with_an_incompatible_bundle_is_a_hard_failure() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());
    write_stale_bundle(temp.path());

    let (result, _) = run_with_failure(not_allowed());

    assert!(result.is_err());
}

#[test]
fn a_fetched_tree_with_the_wrong_version_is_a_hard_failure() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());

    let (result, notice) = run_with_failure(anyhow::Error::new(FetchedWrongVersion {
        expected: crate::VERSION.to_string(),
        found: super::super::framework_source::SourceVersion::Unknown,
    }));

    assert!(result.is_err(), "a wrong tree must never be continued past");
    assert!(!notice.contains("keeping the installed bundle"), "{notice}");
}

#[test]
fn any_other_install_failure_is_a_hard_failure() {
    // A manifest failure in particular: the source was reachable and the copy
    // was wrong, which a retry on the next launch won't paper over.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());

    let (result, notice) = run_with_failure(anyhow::anyhow!(
        "the staged bundle does not match its source /src (1 missing, 0 extra, 0 changed)"
    ));

    assert!(result.is_err());
    assert!(notice.is_empty(), "{notice}");
}

// These pin guarantees this change must keep. They pass before it as well.

#[cfg(unix)]
#[test]
fn the_install_runs_under_the_install_lock() {
    use fs4::fs_std::FileExt;

    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());
    let lock_path = temp.path().join(".amplihack/.install.lock");
    let held = Cell::new(false);

    let mut notice = Vec::new();
    ensure_assets_match_binary_version_with(&launch_args(), &mut notice, || {
        let probe = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)?;
        held.set(!FileExt::try_lock_exclusive(&probe)?);
        version_stamp::write_installed_version(crate::VERSION)
    })
    .unwrap();

    assert!(
        held.get(),
        "the fetch and the stage must hold .install.lock"
    );
}

#[test]
fn an_older_binary_still_never_stages_over_a_newer_stamp() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    create_minimal_staged_assets(temp.path());
    version_stamp::write_installed_version(&newer_version()).unwrap();
    let calls = Cell::new(0u32);

    let mut notice = Vec::new();
    ensure_assets_match_binary_version_with(&launch_args(), &mut notice, || {
        calls.set(calls.get() + 1);
        Err(fetch_failed("must not be reached"))
    })
    .unwrap();

    assert_eq!(calls.get(), 0);
}
