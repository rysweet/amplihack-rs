//! Issue #1955, crusty round 1: a failed implicit tag fetch is remembered, so
//! launches that follow it don't each wait out the fetch budget again.
//!
//! Each test runs the real install path with an isolated HOME and an injected
//! fetcher, so the marker is the production one under `~/.amplihack`.

use super::super::InstallMode;
use super::super::fetch_backoff::{BACKOFF_WINDOW, MARKER_FILE};
use super::super::framework_source::BinaryIdentity;
use super::super::run_install_with_fetcher;
use super::super::tag_fetch::SourceFetchFailed;
use super::issue_1955_support::*;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn release_binary() -> BinaryIdentity {
    BinaryIdentity {
        version: crate::VERSION.to_string(),
        may_fetch: true,
    }
}

/// `force_refresh`, because the compile-time root would match this test build
/// and no fetch would run.
fn install(mode: InstallMode, fetcher: &FakeFetcher) -> anyhow::Result<()> {
    run_install_with_fetcher(None, false, true, false, mode, &release_binary(), fetcher)
}

fn marker(fx: &InstallEnv) -> PathBuf {
    fx.home.join(".amplihack").join(MARKER_FILE)
}

fn write_marker(fx: &InstallEnv, version: &str, failed_at: SystemTime) {
    let secs = failed_at.duration_since(UNIX_EPOCH).unwrap().as_secs();
    fs::create_dir_all(fx.home.join(".amplihack")).unwrap();
    fs::write(marker(fx), format!("{version}\n{secs}\n")).unwrap();
}

fn fetch_failure(err: &anyhow::Error) -> &SourceFetchFailed {
    err.downcast_ref::<SourceFetchFailed>()
        .unwrap_or_else(|| panic!("expected SourceFetchFailed, got {err:#}"))
}

#[test]
fn a_second_launch_after_a_failed_fetch_makes_no_fetch() {
    let fx = InstallEnv::new();
    let fetcher = FakeFetcher::failing("network request timed out");

    let first = install(InstallMode::Implicit, &fetcher).expect_err("the fetch fails");
    assert!(matches!(
        fetch_failure(&first),
        SourceFetchFailed::Failed { .. }
    ));
    assert_eq!(fetcher.call_count(), 1);
    assert!(marker(&fx).is_file(), "the failure must be recorded");

    let second = install(InstallMode::Implicit, &fetcher).expect_err("still no source");
    assert_eq!(fetcher.call_count(), 1, "the second launch must not fetch");
    match fetch_failure(&second) {
        // The same variant, so self-heal keeps a compatible bundle as before.
        SourceFetchFailed::Failed { version, reason } => {
            assert_eq!(version, crate::VERSION);
            assert!(reason.contains("not retried"), "{reason}");
            assert!(!reason.contains('\n'), "{reason}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn an_explicit_install_fetches_despite_a_recent_failure_and_clears_it() {
    let fx = InstallEnv::new();
    write_marker(&fx, crate::VERSION, SystemTime::now());
    let fetcher = FakeFetcher::serving(crate::VERSION);

    install(InstallMode::Explicit, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(
        fetcher.call_count(),
        1,
        "a user who asks for an install gets a fetch"
    );
    assert!(
        !marker(&fx).exists(),
        "a successful fetch forgets the failure"
    );
}

#[test]
fn a_failure_older_than_the_window_is_retried() {
    let fx = InstallEnv::new();
    let long_ago = SystemTime::now() - BACKOFF_WINDOW - std::time::Duration::from_secs(1);
    write_marker(&fx, crate::VERSION, long_ago);
    let fetcher = FakeFetcher::serving(crate::VERSION);

    install(InstallMode::Implicit, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(fetcher.call_count(), 1);
    assert!(!marker(&fx).exists());
}

#[test]
fn a_failure_for_another_version_does_not_block_this_one() {
    let fx = InstallEnv::new();
    write_marker(&fx, &older_than_binary(), SystemTime::now());
    let fetcher = FakeFetcher::serving(crate::VERSION);

    install(InstallMode::Implicit, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(fetcher.call_count(), 1);
}

#[test]
fn a_build_that_may_not_fetch_records_nothing() {
    let fx = InstallEnv::new();
    let fetcher = FakeFetcher::failing("must not be called");
    let binary = BinaryIdentity {
        version: crate::VERSION.to_string(),
        may_fetch: false,
    };

    let err = run_install_with_fetcher(
        None,
        false,
        true,
        false,
        InstallMode::Implicit,
        &binary,
        &fetcher,
    )
    .expect_err("no source");

    assert!(matches!(
        fetch_failure(&err),
        SourceFetchFailed::NotAllowed { .. }
    ));
    assert_eq!(fetcher.call_count(), 0);
    assert!(!marker(&fx).exists());
}
