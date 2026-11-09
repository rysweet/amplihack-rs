//! Issue #1990: when the new amplifier-bundle cannot be swapped in and the
//! previous bundle cannot be rolled back either, the error must say so and
//! name `amplifier-bundle.old`, where the previous bundle was left.
//!
//! Every path lives under `tempdir_outside_framework_roots()`; HOME, the real
//! `~/.claude` and the real `~/.amplihack` are never read or written.

use super::super::directories::{copy_amplifier_bundle, staging_hook::with_rename_failures};
use super::helpers::*;
use super::issue_1526_implicit_downgrade::snapshot;
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const SWAP_FAILURE: &str = "injected swap failure";
const ROLLBACK_FAILURE: &str = "injected rollback failure";
const SWAP_PREFIX: &str = "failed to swap new amplifier-bundle into";

struct Staging {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    claude_dir: PathBuf,
    target: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
}

/// A source repo plus an existing installed bundle with a sentinel in it.
fn staging_with_installed_bundle() -> Staging {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let repo = temp.path().join("repo");
    create_source_repo(&repo);
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();
    let target = temp.path().join(".amplihack/amplifier-bundle");
    let staging = temp.path().join(".amplihack/amplifier-bundle.staging");
    let backup = target.with_extension("old");
    copy_amplifier_bundle(&repo, &claude_dir).unwrap();
    fs::write(target.join("previous-install-sentinel"), "keep me\n").unwrap();
    Staging {
        _temp: temp,
        repo,
        claude_dir,
        target,
        staging,
        backup,
    }
}

#[test]
fn failed_rollback_reports_both_errors_and_backup_path() {
    let fixture = staging_with_installed_bundle();
    let (staging, backup, target) = (
        fixture.staging.clone(),
        fixture.backup.clone(),
        fixture.target.clone(),
    );
    let calls: Rc<RefCell<Vec<(PathBuf, PathBuf)>>> = Rc::default();
    let seen = Rc::clone(&calls);

    let result = with_rename_failures(
        move |from: &Path, to: &Path| {
            seen.borrow_mut()
                .push((from.to_path_buf(), to.to_path_buf()));
            if from == staging {
                Some(io::Error::other(SWAP_FAILURE))
            } else if from == backup {
                Some(io::Error::other(ROLLBACK_FAILURE))
            } else {
                None
            }
        },
        || copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir),
    );

    let err = result.expect_err("a failed swap must fail the install");
    let text = format!("{err:#}");
    assert!(text.contains(SWAP_PREFIX), "missing swap prefix: {text}");
    assert!(text.contains(SWAP_FAILURE), "missing swap error: {text}");
    assert!(
        text.contains(ROLLBACK_FAILURE),
        "missing rollback error: {text}"
    );
    assert!(
        text.contains("amplifier-bundle.old"),
        "the error must name amplifier-bundle.old: {text}"
    );
    assert!(
        text.contains(&fixture.backup.display().to_string()),
        "the error must give the full backup path {}: {text}",
        fixture.backup.display()
    );

    assert_eq!(
        *calls.borrow(),
        vec![
            (fixture.staging.clone(), target.clone()),
            (fixture.backup.clone(), target),
        ],
        "only the swap and then the rollback go through the seam"
    );
    assert!(
        fixture.backup.join("previous-install-sentinel").is_file(),
        "the previous bundle must be left intact at amplifier-bundle.old"
    );
    assert!(
        !fixture.target.exists(),
        "nothing may be put in place of the bundle that could not be restored"
    );
    assert!(
        !fixture.staging.exists(),
        "staging must be removed on failure"
    );
}

#[test]
fn successful_rollback_keeps_existing_error_and_restores_bundle() {
    let fixture = staging_with_installed_bundle();
    let before = snapshot(&fixture.target);
    let staging = fixture.staging.clone();

    let result = with_rename_failures(
        move |from: &Path, _to: &Path| (from == staging).then(|| io::Error::other(SWAP_FAILURE)),
        || copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir),
    );

    let err = result.expect_err("a failed swap must fail the install");
    let text = format!("{err:#}");
    assert_eq!(
        text,
        format!("{SWAP_PREFIX} {}: {SWAP_FAILURE}", fixture.target.display()),
        "the single-failure message must stay unchanged"
    );
    assert!(!text.contains("could not restore"), "{text}");

    assert_eq!(
        snapshot(&fixture.target),
        before,
        "the previous bundle must be restored byte for byte"
    );
    assert!(
        !fixture.backup.exists(),
        "a successful rollback leaves no amplifier-bundle.old"
    );
    assert!(
        !fixture.staging.exists(),
        "staging must be removed on failure"
    );
}
