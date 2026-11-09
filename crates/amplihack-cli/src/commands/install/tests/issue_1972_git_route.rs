//! Issue #1972, crusty round 1: the git route of the tag fetch must produce
//! the bytes the release manifest lists, whatever the user's git config says,
//! and a git tree that still differs must not be reported as a bad release
//! until the tag tarball differs too.
//!
//! The real-git test fetches from a local repository through a `url.insteadOf`
//! in its own HOME, with the conversions a gitconfig shared with Windows turns
//! on. The others use a fake `git` that copies a prepared tree into place.

use super::super::InstallMode;
use super::super::clone::{FrameworkRootQuery, which_git_in};
use super::super::framework_source::*;
use super::super::release_manifest::ReleaseManifest;
use super::super::tag_fetch::*;
use super::issue_1955_support::*;
use super::issue_1955_tag_fetch::{
    LONG, REPO_URL, gz_of, log, recording_git, recording_http, refused_http,
};
use super::issue_1972_support::*;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A bundle file holding `x\n`, which line-ending conversion rewrites.
const TEXT_FILE: &str = "amplifier-bundle/agents/marker.txt";

fn policy(mode: InstallMode, backoff: Option<PathBuf>) -> SourcePolicy {
    SourcePolicy {
        local: None,
        force_refresh: true,
        mode,
        fetch_backoff: backoff,
    }
}

fn acquire(
    policy: &SourcePolicy,
    release: &ReleaseManifest,
    fetcher: &ReleaseTagFetcher,
) -> anyhow::Result<AcquiredSource> {
    acquire_framework_source(
        policy,
        &FrameworkRootQuery::default(),
        &release_binary(),
        Some(release),
        fetcher,
        &accept_all,
    )
}

/// The release tree: what CI hashed and what the tag tarball holds.
fn release_tree(root: &Path) -> PathBuf {
    versioned_tree(root, BINARY);
    std::os::unix::fs::symlink(
        "marker.txt",
        root.join("amplifier-bundle/agents/link-to-marker"),
    )
    .unwrap();
    root.to_path_buf()
}

/// A tag tarball of `tree`, under one top-level directory as GitHub makes it.
fn tarball_of(tree: &Path) -> Vec<u8> {
    gz_of(|builder| {
        builder
            .append_dir_all(format!("amplihack-rs-{BINARY}"), tree)
            .unwrap();
    })
}

/// A fake `git` whose every call copies `tree` into its working directory, so
/// the git route "succeeds" with exactly that tree.
fn git_serving(dir: &Path, tree: &Path) -> PathBuf {
    recording_git(dir, &format!("cp -R '{}/.' .", tree.display()))
}

/// The release tree as a CRLF-converting checkout writes it.
fn crlf_copy_of(at: &Path) -> PathBuf {
    release_tree(at);
    fs::write(at.join(TEXT_FILE), "x\r\n").unwrap();
    at.to_path_buf()
}

// ── The real git route under a converting user config ──

/// A real `git`, found under the lock `InstallEnv` holds while it changes PATH.
fn real_git() -> PathBuf {
    let _path = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    which_git_in(&std::env::var_os("PATH").unwrap()).unwrap()
}

fn run_git(git: &Path, home: &Path, dir: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new(git)
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_GLOBAL")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// A HOME whose `.gitconfig` sends the release URL to a local repository
/// tagged `v1.2.99`, and turns on every conversion that rewrites a checkout.
/// The config is written after the commit, so the repository holds LF bytes.
fn converting_home(git: &Path, home: &Path) -> PathBuf {
    let upstream = home.join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    run_git(git, home, &upstream, &["init", "--quiet"]);
    release_tree(&upstream);
    run_git(git, home, &upstream, &["add", "-A"]);
    run_git(
        git,
        home,
        &upstream,
        &["commit", "--quiet", "--no-gpg-sign", "-m", "release"],
    );
    run_git(
        git,
        home,
        &upstream,
        &["tag", format!("v{BINARY}").as_str()],
    );
    let attributes = home.join("global-attributes");
    fs::write(&attributes, "* text eol=crlf\n").unwrap();
    fs::write(
        home.join(".gitconfig"),
        format!(
            "[url \"file://{}\"]\n\tinsteadOf = {REPO_URL}\n\
             [core]\n\tautocrlf = true\n\teol = crlf\n\tsymlinks = false\n\
             \tattributesFile = {}\n",
            upstream.display(),
            attributes.display()
        ),
    )
    .unwrap();
    upstream
}

#[test]
fn issue_1972_the_fixture_config_rewrites_a_plain_git_checkout() {
    // Without this, the next test could pass on a config that changes nothing.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = real_git();
    let upstream = converting_home(&git, temp.path());
    let plain = temp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();

    run_git(&git, temp.path(), &plain, &["init", "--quiet"]);
    run_git(
        &git,
        temp.path(),
        &plain,
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--",
            REPO_URL,
            "refs/tags/v1.2.99",
        ],
    );
    run_git(
        &git,
        temp.path(),
        &plain,
        &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
    );

    assert_eq!(fs::read(upstream.join(TEXT_FILE)).unwrap(), b"x\n");
    assert_eq!(fs::read(plain.join(TEXT_FILE)).unwrap(), b"x\r\n");
    assert!(
        fs::symlink_metadata(plain.join("amplifier-bundle/agents/link-to-marker"))
            .unwrap()
            .is_file(),
        "core.symlinks=false writes the symlink as a plain file"
    );
}

#[test]
fn issue_1972_a_converting_user_git_config_still_gives_the_release_bytes() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = real_git();
    let upstream = converting_home(&git, temp.path());
    let release = release_of(&upstream, BINARY);
    let (http_calls, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let source = crate::test_support::with_env(
        &[
            ("HOME", Some(temp.path().as_os_str())),
            ("GIT_CONFIG_NOSYSTEM", Some(OsStr::new("1"))),
            ("GIT_CONFIG_GLOBAL", None),
        ],
        || acquire(&policy(InstallMode::Explicit, None), &release, &fetcher),
    )
    .unwrap_or_else(|e| panic!("the git route must give the release bytes: {e:#}"));

    assert!(
        http_calls.calls.borrow().is_empty(),
        "the git tree matched, so the tarball must not be needed"
    );
    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    assert_eq!(fs::read(source.root.join(TEXT_FILE)).unwrap(), b"x\n");
}

#[test]
fn issue_1972_every_git_call_turns_off_checkout_conversions() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 0");
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let _ = fetcher.fetch(
        &semver::Version::parse(BINARY).unwrap(),
        &temp.path().join("dest"),
        LONG,
    );

    let argv = log(temp.path(), "argv.log");
    assert_eq!(argv.lines().count(), 3, "init, fetch and checkout:\n{argv}");
    for line in argv.lines() {
        for setting in [
            "-c core.autocrlf=false",
            "-c core.eol=lf",
            "-c core.attributesFile=/dev/null",
            "-c core.symlinks=true",
        ] {
            assert!(line.contains(setting), "{setting:?} missing from: {line}");
        }
    }
}

// ── A git tree that fails the manifest goes to the tarball ──

#[test]
fn issue_1972_a_git_tree_that_differs_is_replaced_by_a_matching_tarball() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let pristine = release_tree(&temp.path().join("release"));
    let release = release_of(&pristine, BINARY);
    let converted = crlf_copy_of(&temp.path().join("converted"));
    let archive = tarball_of(&pristine);
    let (http_calls, http) = recording_http(move || Ok(archive.clone()));
    let fetcher = ReleaseTagFetcher {
        git: Some(git_serving(temp.path(), &converted)),
        http,
    };

    for mode in [InstallMode::Explicit, InstallMode::Implicit] {
        let marker = temp.path().join(format!("{mode:?}.tag-fetch-failed"));
        let source = acquire(&policy(mode, Some(marker.clone())), &release, &fetcher)
            .unwrap_or_else(|e| panic!("the tarball matches, so this must succeed: {e:#}"));

        assert_eq!(source.origin, SourceOrigin::ReleaseTag);
        assert_eq!(fs::read(source.root.join(TEXT_FILE)).unwrap(), b"x\n");
        assert!(
            !source.root.ends_with("repo"),
            "the git tree must not be the one used: {}",
            source.root.display()
        );
        assert!(!marker.exists(), "a successful fetch leaves no backoff");
    }
    assert_eq!(http_calls.calls.borrow().len(), 2, "one tarball per fetch");
}

#[test]
fn issue_1972_a_differing_git_tree_with_no_tarball_is_a_network_failure() {
    // The release has not been shown to be wrong, so self-heal may keep the
    // installed bundle, the launch backs off, and nothing asks for a report.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let pristine = release_tree(&temp.path().join("release"));
    let release = release_of(&pristine, BINARY);
    let converted = crlf_copy_of(&temp.path().join("converted"));
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git_serving(temp.path(), &converted)),
        http,
    };
    let marker = temp.path().join(".tag-fetch-failed");

    let err = acquire(
        &policy(InstallMode::Implicit, Some(marker.clone())),
        &release,
        &fetcher,
    )
    .expect_err("neither route gave the release bytes");

    assert!(err.downcast_ref::<FetchedManifestMismatch>().is_none());
    let Some(SourceFetchFailed::Failed { reason, .. }) = err.downcast_ref::<SourceFetchFailed>()
    else {
        panic!("expected SourceFetchFailed::Failed, got {err:#}");
    };
    assert!(
        reason.contains(
            "git: the checked-out tree differs from the release manifest \
             (0 missing, 0 extra, 1 changed)"
        ),
        "{reason}"
    );
    assert!(reason.contains("tarball: offline"), "{reason}");
    assert!(!format!("{err:#}").contains("Please report"), "{err:#}");
    assert!(marker.exists(), "the launch must back off");
}

#[test]
fn issue_1972_only_a_tarball_that_differs_too_blames_the_release() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let pristine = release_tree(&temp.path().join("release"));
    let release = release_of(&pristine, BINARY);
    let converted = crlf_copy_of(&temp.path().join("converted"));
    let changed = release_tree(&temp.path().join("changed"));
    change_file(&changed, LISTED);
    let archive = tarball_of(&changed);
    let (http_calls, http) = recording_http(move || Ok(archive.clone()));
    let fetcher = ReleaseTagFetcher {
        git: Some(git_serving(temp.path(), &converted)),
        http,
    };
    let marker = temp.path().join(".tag-fetch-failed");

    let err = acquire(
        &policy(InstallMode::Implicit, Some(marker.clone())),
        &release,
        &fetcher,
    )
    .expect_err("the tag's own bytes differ from the release");

    let mismatch = err
        .downcast_ref::<FetchedManifestMismatch>()
        .unwrap_or_else(|| panic!("expected FetchedManifestMismatch, got {err:#}"));
    assert_eq!(mismatch.diff.changed, vec![LISTED.to_string()]);
    assert!(format!("{err:#}").contains("Please report"), "{err:#}");
    assert!(err.downcast_ref::<SourceFetchFailed>().is_none());
    assert_eq!(http_calls.calls.borrow().len(), 1);
    assert!(!marker.exists(), "not a network failure, so no backoff");
}

#[test]
fn issue_1972_a_git_tree_at_another_version_does_not_go_to_the_tarball() {
    // A wrong version is the tag's own, so the tarball would say the same.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let pristine = release_tree(&temp.path().join("release"));
    let release = release_of(&pristine, BINARY);
    let other = release_tree(&temp.path().join("other"));
    write_workspace_toml(&other, "9.9.9");
    let (http_calls, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git_serving(temp.path(), &other)),
        http,
    };

    let err = acquire(&policy(InstallMode::Explicit, None), &release, &fetcher)
        .expect_err("a tree at another version is never staged");

    assert!(
        err.downcast_ref::<FetchedWrongVersion>().is_some(),
        "{err:#}"
    );
    assert!(http_calls.calls.borrow().is_empty());
}
