//! Issue #1972 R11, D14: the staged copy against the release manifest, at the
//! staging layer and through a real install.

use super::super::InstallMode;
use super::super::directories::{copy_amplifier_bundle, staging_hook::with_staging_hook};
use super::super::framework_source::{BinaryIdentity, ReleaseManifestRefused};
use super::super::release_manifest::ReleaseManifest;
use super::super::run_install_with_manifest;
use super::super::tag_fetch::FetchedManifestMismatch;
use super::super::tag_fetch::TagFetcher;
use super::super::version_stamp;
use super::helpers::create_source_repo;
use super::issue_1526_implicit_downgrade::snapshot;
use super::issue_1955_support::*;
use super::issue_1972_support::*;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ── Staging layer ──

struct Staging {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    claude_dir: PathBuf,
    target: PathBuf,
    staging: PathBuf,
    release: ReleaseManifest,
}

/// A source repo, its release manifest, and an installed bundle with a
/// sentinel in it.
fn staging() -> Staging {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let repo = temp.path().join("repo");
    create_source_repo(&repo);
    let release = release_of(&repo, BINARY);
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();
    let target = temp.path().join(".amplihack/amplifier-bundle");
    let staging = temp.path().join(".amplihack/amplifier-bundle.staging");
    copy_amplifier_bundle(&repo, &claude_dir, Some(&release)).unwrap();
    fs::write(target.join("previous-install-sentinel"), "keep me\n").unwrap();
    Staging {
        _temp: temp,
        repo,
        claude_dir,
        target,
        staging,
        release,
    }
}

#[test]
fn issue_1972_a_matching_copy_is_swapped_in() {
    let fx = staging();
    copy_amplifier_bundle(&fx.repo, &fx.claude_dir, Some(&fx.release)).unwrap();
    assert!(!fx.target.join("previous-install-sentinel").exists());
    assert!(!fx.staging.exists());
}

#[test]
fn issue_1972_a_source_changed_after_the_gate_is_caught_before_the_swap() {
    // The source and staging both change, as when the source is edited
    // between the gate and the copy: the source check passes, R11 does not.
    let fx = staging();
    let before = snapshot(&fx.target);
    let repo = fx.repo.clone();
    let tamper = move |staging: &Path| {
        fs::write(repo.join("amplifier-bundle").join(LISTED), "edited\n").unwrap();
        fs::write(staging.join(LISTED), "edited\n").unwrap();
    };

    let err = with_staging_hook(tamper, || {
        copy_amplifier_bundle(&fx.repo, &fx.claude_dir, Some(&fx.release))
    })
    .expect_err("R11 must catch it");

    let text = format!("{err:#}");
    for phrase in [
        "the staged copy at",
        &format!("does not match the release v{BINARY} manifest embedded in this binary"),
        &format!("changed: {LISTED}"),
        "Nothing was swapped in; the installed bundle is unchanged.",
    ] {
        assert!(text.contains(phrase), "missing {phrase:?} in:\n{text}");
    }
    assert_eq!(snapshot(&fx.target), before, "previous bundle intact");
    assert!(!fx.staging.exists(), "staging removed");
}

#[test]
fn issue_1972_the_copy_against_source_message_comes_first() {
    let fx = staging();
    let tamper = |staging: &Path| fs::write(staging.join("tools/smuggled.sh"), "x").unwrap();

    let err = with_staging_hook(tamper, || {
        copy_amplifier_bundle(&fx.repo, &fx.claude_dir, Some(&fx.release))
    })
    .unwrap_err();

    let text = format!("{err:#}");
    assert!(text.contains("does not match its source"), "{text}");
    assert!(!text.contains("release v"), "{text}");
}

#[test]
fn issue_1972_an_excluded_name_in_staging_counts_as_extra() {
    // `of_staged` lists everything; the source check catches it first, as
    // before. Without a source check (impossible in production) R11 would too.
    let fx = staging();
    let tamper = |staging: &Path| {
        fs::create_dir_all(staging.join("tools/__pycache__")).unwrap();
        fs::write(staging.join("tools/__pycache__/x.pyc"), "x").unwrap();
    };
    let err = with_staging_hook(tamper, || {
        copy_amplifier_bundle(&fx.repo, &fx.claude_dir, Some(&fx.release))
    })
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("tools/__pycache__/x.pyc"),
        "{err:#}"
    );
}

#[test]
fn issue_1972_no_expected_manifest_means_no_release_check() {
    let fx = staging();
    change_file(&fx.repo, LISTED);
    copy_amplifier_bundle(&fx.repo, &fx.claude_dir, None).unwrap();
    assert_eq!(
        fs::read_to_string(fx.target.join(LISTED)).unwrap(),
        "changed after release\n"
    );
}

// ── Through a real install ──

fn release_binary_now() -> BinaryIdentity {
    BinaryIdentity {
        version: crate::VERSION.to_string(),
        may_fetch: true,
    }
}

/// `--force-refresh` keeps the test process's own checkout (the cwd and the
/// compile-time root) out of resolution, so a release-tag test reaches the tag.
fn install(
    local: Option<PathBuf>,
    force_refresh: bool,
    release: &ReleaseManifest,
    fetcher: &FakeFetcher,
) -> anyhow::Result<()> {
    run_install_with_manifest(
        local,
        false,
        force_refresh,
        false,
        InstallMode::Explicit,
        &release_binary_now(),
        Some(release),
        fetcher,
    )
}

#[test]
fn issue_1972_r11_applies_to_a_resolved_source() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("src"), crate::VERSION);
    let release = release_of(&tree, crate::VERSION);
    fx.env.set("AMPLIHACK_HOME", &tree);
    let edit_source = tree.join("amplifier-bundle").join(LISTED);
    let tamper = move |staging: &Path| {
        fs::write(&edit_source, "edited\n").unwrap();
        fs::write(staging.join(LISTED), "edited\n").unwrap();
    };

    let err = with_staging_hook(tamper, || {
        install(None, false, &release, &FakeFetcher::failing("unused"))
    })
    .expect_err("R11 must fail the install");

    assert!(format!("{err:#}").contains("the staged copy at"), "{err:#}");
    assert!(!fx.staged_bundle().exists(), "nothing swapped in");
}

#[test]
fn issue_1972_r11_applies_to_a_release_tag_source() {
    let _fx = InstallEnv::new();
    let release = served_release(crate::VERSION);
    let fetched: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let fetcher = {
        let fetched = Rc::clone(&fetched);
        FakeFetcher::new(move |destination| {
            let root = FakeFetcher::serving(crate::VERSION).fetch(
                &semver::Version::parse(crate::VERSION).unwrap(),
                destination,
                std::time::Duration::from_secs(1),
            )?;
            *fetched.borrow_mut() = Some(root.clone());
            Ok(root)
        })
    };
    let tamper = {
        let fetched = Rc::clone(&fetched);
        move |staging: &Path| {
            let root = fetched.borrow().clone().unwrap();
            fs::write(root.join("amplifier-bundle").join(LISTED), "edited\n").unwrap();
            fs::write(staging.join(LISTED), "edited\n").unwrap();
        }
    };

    let err = with_staging_hook(tamper, || install(None, true, &release, &fetcher))
        .expect_err("R11 must fail the install");

    assert!(format!("{err:#}").contains("the staged copy at"), "{err:#}");
}

#[test]
fn issue_1972_local_stages_a_non_matching_tree() {
    let fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("src"), crate::VERSION);
    let release = release_of(&tree, crate::VERSION);
    change_file(&tree, LISTED);

    install(Some(tree), false, &release, &FakeFetcher::failing("unused"))
        .unwrap_or_else(|e| panic!("--local must stage: {e:#}"));

    assert_eq!(
        fs::read_to_string(fx.staged_file(LISTED)).unwrap(),
        "changed after release\n"
    );
}

#[test]
fn issue_1972_a_refusal_or_a_mismatched_fetch_changes_nothing_under_home() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("src"), crate::VERSION);
    let release = release_of(&tree, crate::VERSION);
    change_file(&tree, LISTED);
    fx.env.set("AMPLIHACK_HOME", &tree);
    let before = snapshot(&fx.home);

    let err = install(None, false, &release, &FakeFetcher::failing("unused")).unwrap_err();
    assert!(
        err.downcast_ref::<ReleaseManifestRefused>().is_some(),
        "{err:#}"
    );
    assert_eq!(snapshot(&fx.home), before, "a refusal deploys nothing");

    fx.env.unset("AMPLIHACK_HOME");
    let written = Rc::new(RefCell::new(None));
    let err = install(
        None,
        true,
        &served_release(crate::VERSION),
        &tampered_fetcher(crate::VERSION, written),
    )
    .unwrap_err();
    assert!(
        err.downcast_ref::<FetchedManifestMismatch>().is_some(),
        "{err:#}"
    );
    assert_eq!(
        snapshot(&fx.home),
        before,
        "a mismatched fetch deploys nothing"
    );
    assert_eq!(version_stamp::read_installed_version_quiet().unwrap(), None);
}
