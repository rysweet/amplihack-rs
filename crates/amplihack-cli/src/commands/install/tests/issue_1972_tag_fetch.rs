//! Issue #1972 R8, D11, D12, D17: a fetched release tag tree against the
//! release manifest embedded in the binary, and what self-heal does when it
//! does not match.

use super::super::InstallMode;
use super::super::bundle_manifest::ManifestDiff;
use super::super::clone::FrameworkRootQuery;
use super::super::framework_source::*;
use super::super::release_manifest::ReleaseManifest;
use super::super::tag_fetch::{
    FetchedManifestMismatch, FetchedWrongVersion, SourceFetchFailed, TagFetcher,
};
use super::super::version_stamp;
use super::helpers::create_minimal_staged_assets;
use super::issue_1526_implicit_downgrade::{IsolatedHome, args};
use super::issue_1955_support::*;
use super::issue_1972_support::*;
use crate::self_heal::ensure_assets_match_binary_version_with;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn policy(mode: InstallMode, backoff: Option<PathBuf>) -> SourcePolicy {
    SourcePolicy {
        local: None,
        force_refresh: false,
        mode,
        fetch_backoff: backoff,
    }
}

fn acquire(
    policy: &SourcePolicy,
    release: &ReleaseManifest,
    fetcher: &FakeFetcher,
) -> anyhow::Result<AcquiredSource> {
    acquire_framework_source_with_manifest(
        policy,
        &FrameworkRootQuery::default(),
        &release_binary(),
        Some(release),
        fetcher,
        &accept_all,
    )
}

#[test]
fn issue_1972_a_matching_tag_tree_is_accepted() {
    // AC4.
    for mode in [InstallMode::Explicit, InstallMode::Implicit] {
        let fetcher = FakeFetcher::serving(BINARY);
        let source = acquire(&policy(mode, None), &served_release(BINARY), &fetcher).unwrap();
        assert_eq!(source.origin, SourceOrigin::ReleaseTag);
        assert_eq!(fetcher.call_count(), 1);
    }
}

#[test]
fn issue_1972_a_changed_tag_tree_is_a_hard_failure_with_no_backoff() {
    // AC3, in both modes, with the backoff enabled.
    for mode in [InstallMode::Explicit, InstallMode::Implicit] {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let marker = temp.path().join(".tag-fetch-failed");
        let written = Rc::new(RefCell::new(None));
        let fetcher = tampered_fetcher(BINARY, Rc::clone(&written));

        let err = acquire(
            &policy(mode, Some(marker.clone())),
            &served_release(BINARY),
            &fetcher,
        )
        .expect_err("a changed tag tree must not be staged");

        assert!(
            err.downcast_ref::<SourceFetchFailed>().is_none(),
            "self-heal must not keep the old bundle: {err:#}"
        );
        let mismatch = err
            .downcast_ref::<FetchedManifestMismatch>()
            .unwrap_or_else(|| panic!("expected FetchedManifestMismatch, got {err:#}"));
        assert_eq!(mismatch.version, BINARY);
        assert_eq!(mismatch.diff.changed, [LISTED]);
        assert!(!marker.exists(), "no backoff marker for a content mismatch");
        let tree = written.borrow().clone().expect("the fetcher ran");
        assert!(
            !tree.exists(),
            "the fetched tree is removed: {}",
            tree.display()
        );

        let text = err.to_string();
        for phrase in [
            &format!(
                "the tree fetched for release tag v{BINARY} does not match the release v{BINARY} manifest embedded in this binary"
            ),
            &format!("changed: {LISTED}"),
            "This is not a network failure",
            "nothing was staged",
            "https://github.com/rysweet/amplihack-rs/issues",
        ] {
            assert!(text.contains(phrase), "missing {phrase:?} in:\n{text}");
        }
    }
}

#[test]
fn issue_1972_the_version_check_comes_before_the_manifest_check() {
    let fetcher = FakeFetcher::serving("1.2.0");
    let err = acquire(
        &policy(InstallMode::Explicit, None),
        &served_release(BINARY),
        &fetcher,
    )
    .unwrap_err();
    assert!(
        err.downcast_ref::<FetchedWrongVersion>().is_some(),
        "{err:#}"
    );
}

#[test]
fn issue_1972_the_manifest_check_comes_before_validation() {
    let written = Rc::new(RefCell::new(None));
    let fetcher = tampered_fetcher(BINARY, written);
    let err = acquire_framework_source_with_manifest(
        &policy(InstallMode::Explicit, None),
        &FrameworkRootQuery::default(),
        &release_binary(),
        Some(&served_release(BINARY)),
        &fetcher,
        &|_: &Path| Err("validation must not run".to_string()),
    )
    .unwrap_err();
    assert!(
        err.downcast_ref::<FetchedManifestMismatch>().is_some(),
        "{err:#}"
    );
}

#[cfg(unix)]
#[test]
fn issue_1972_a_hashing_error_on_the_fetched_tree_is_a_hard_failure() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: running as root");
        return;
    }
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let marker = temp.path().join(".tag-fetch-failed");
    let fetcher = FakeFetcher::new(|destination| {
        let root = FakeFetcher::serving(BINARY).fetch(
            &semver::Version::parse(BINARY).unwrap(),
            destination,
            std::time::Duration::from_secs(1),
        )?;
        let file = root.join("amplifier-bundle").join(LISTED);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        Ok(root)
    });

    let err = acquire(
        &policy(InstallMode::Implicit, Some(marker.clone())),
        &served_release(BINARY),
        &fetcher,
    )
    .unwrap_err();

    assert!(err.downcast_ref::<SourceFetchFailed>().is_none(), "{err:#}");
    assert!(err.downcast_ref::<FetchedManifestMismatch>().is_none());
    assert!(
        err.to_string()
            .contains("could not hash the tree fetched for release tag"),
        "{err:#}"
    );
    assert!(!marker.exists());
}

#[test]
fn issue_1972_without_a_manifest_a_changed_tag_tree_is_staged_as_before() {
    let written = Rc::new(RefCell::new(None));
    let fetcher = tampered_fetcher(BINARY, written);
    let source = acquire_framework_source_with_manifest(
        &policy(InstallMode::Explicit, None),
        &FrameworkRootQuery::default(),
        &release_binary(),
        None,
        &fetcher,
        &accept_all,
    )
    .unwrap();
    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
}

// ── AC7: self-heal and the launch bootstrap ──

fn stale_install(home: &Path) {
    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(&older_than_binary()).unwrap();
}

fn run_self_heal(error: anyhow::Error) -> (anyhow::Result<()>, String) {
    let mut notice = Vec::new();
    let result = ensure_assets_match_binary_version_with(
        &args(&["amplihack", "launch"]),
        &mut notice,
        || Err(error),
    );
    (result, String::from_utf8(notice).unwrap())
}

#[test]
fn issue_1972_self_heal_exits_on_a_fetched_manifest_mismatch() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());

    let (result, notice) = run_self_heal(anyhow::Error::new(FetchedManifestMismatch {
        version: crate::VERSION.to_string(),
        diff: ManifestDiff {
            changed: vec![LISTED.to_string()],
            ..ManifestDiff::default()
        },
    }));

    let err = result.expect_err("a tampered tag must never be continued past");
    assert!(
        format!("{err:#}").contains("This is not a network failure"),
        "{err:#}"
    );
    assert!(!notice.contains("keeping the installed bundle"), "{notice}");
}

#[test]
fn issue_1972_self_heal_exits_on_a_malformed_embedded_manifest() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let _env = IsolatedHome::new(temp.path());
    stale_install(temp.path());
    let malformed = ReleaseManifest::from_text(
        "junk\n",
        &BinaryIdentity {
            version: crate::VERSION.to_string(),
            may_fetch: true,
        },
    )
    .unwrap_err();

    let (result, notice) = run_self_heal(anyhow::Error::new(malformed));

    assert!(result.is_err());
    assert!(!notice.contains("keeping the installed bundle"), "{notice}");
}

#[test]
fn issue_1972_the_launch_bootstrap_path_refuses_a_changed_tag_in_implicit_mode() {
    // The launch bootstrap reaches the source decision through
    // `repair_framework_assets` in Implicit mode with the backoff enabled.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let marker = temp.path().join(".tag-fetch-failed");
    let home = temp.path().join("home");
    staged_install(&home, Some("1.2.0"));
    let query = FrameworkRootQuery {
        home: Some(home),
        ..FrameworkRootQuery::default()
    };
    let written = Rc::new(RefCell::new(None));
    let fetcher = tampered_fetcher(BINARY, written);

    let err = acquire_framework_source_with_manifest(
        &policy(InstallMode::Implicit, Some(marker.clone())),
        &query,
        &release_binary(),
        Some(&served_release(BINARY)),
        &fetcher,
        &accept_all,
    )
    .unwrap_err();

    assert!(
        err.downcast_ref::<FetchedManifestMismatch>().is_some(),
        "{err:#}"
    );
    assert!(!marker.exists());
}
