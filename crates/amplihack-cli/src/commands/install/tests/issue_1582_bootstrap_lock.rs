//! Issue #1582: the launch bootstrap's re-stage re-checks under the install
//! lock, through the same helper as startup self-heal.
//!
//! Before the fix, `ensure_framework_installed_with` decided whether the
//! framework assets needed a re-stage, and checked the install stamp, before
//! it took `~/.amplihack/.install.lock`. Two launches that started together
//! both decided to re-stage, then ran one after the other, so the second
//! re-staged again over a fresh install and under live sessions. A newer
//! install that finished while a launch waited for the lock was not seen.
//!
//! None of these tests sleeps, times out or retries. The concurrency test
//! orders its two threads with a barrier, and asserts that the lock is not
//! held before it waits there, so a regression panics instead of hanging.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::{
    IsolatedHome, assert_home_is, assert_refusal_warning, changes, counting_installer,
    newer_version, snapshot,
};
use super::*;
use crate::commands::install::tag_fetch::SourceFetchFailed;
use crate::install_lock;
use crate::self_heal::{LockedRestage, restage_under_install_lock};
use std::cell::Cell;
use std::fs;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU32, Ordering};

/// AC3. Two bootstraps on one home both find the assets missing before either
/// takes the lock. The barrier holds each thread until both have made that
/// unlocked decision, which is exactly the race from the 12 Nov incident.
/// Under the lock the second one re-checks, finds the first one's assets, and
/// installs nothing.
#[test]
fn two_bootstraps_started_together_restage_once() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _env = IsolatedHome::new(home);
    assert_home_is(home);
    assert!(
        framework_restage_needed_now().unwrap(),
        "precondition: an empty home needs a re-stage"
    );

    let barrier = Barrier::new(2);
    let installs = AtomicU32::new(0);
    let launch = || {
        let mut notice = Vec::new();
        let mut first = true;
        let needed = || {
            if !std::mem::take(&mut first) {
                return framework_restage_needed_now();
            }
            // Before the barrier: a bootstrap that took the lock before its
            // first check would otherwise wait here for a thread that is
            // itself waiting for the lock.
            assert!(
                !install_lock::held_by_current_thread(),
                "the bootstrap's first check must run without the install lock"
            );
            let verdict = framework_restage_needed_now();
            // An error is returned only after the barrier, so one thread's
            // failure cannot leave the other waiting.
            barrier.wait();
            verdict
        };
        let install = || {
            installs.fetch_add(1, Ordering::SeqCst);
            create_minimal_staged_assets(home);
            version_stamp::write_installed_version(crate::VERSION)
        };
        let outcome = restage_framework_if_needed(&mut notice, needed, install);
        (outcome, notice)
    };

    let (first, second) = std::thread::scope(|scope| {
        let a = scope.spawn(launch);
        let b = scope.spawn(launch);
        (a.join(), b.join())
    });
    let (first, first_notice) = first.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let (second, second_notice) = second.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let first = first.expect("the first bootstrap's re-stage failed");
    let second = second.expect("the second bootstrap's re-stage failed");

    assert_eq!(
        installs.load(Ordering::SeqCst),
        1,
        "two launches that both found the assets missing must re-stage once: the one \
         that waited for the lock must re-check under it and find nothing to do \
         (outcomes {first:?} and {second:?})"
    );
    assert!(
        matches!(
            (first, second),
            (LockedRestage::Installed, LockedRestage::NotNeeded)
                | (LockedRestage::NotNeeded, LockedRestage::Installed)
        ),
        "one launch installs and the other finds nothing to do; got {first:?} and {second:?}"
    );
    assert_eq!(
        (
            String::from_utf8_lossy(&first_notice),
            String::from_utf8_lossy(&second_notice)
        ),
        ("".into(), "".into()),
        "the shared helper prints no notice of its own on these paths"
    );
    assert!(
        !framework_restage_needed_now().unwrap(),
        "the assets are complete after the two launches"
    );
}

/// AC4. A newer install finishes after the bootstrap's unlocked check and
/// before it holds the lock. Under the lock Guard #2 must see the newer stamp
/// before the assets are re-checked, and the whole bootstrap must return
/// without staging, topping up commands or repairing settings.json.
///
/// There is no hooks binary, so a bootstrap that fell through to its later
/// steps would fail at `stable_hooks_binary` instead of returning `Ok(())`.
fn newer_stamp_found_under_the_lock(bypass: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let mut env = IsolatedHome::new(home);
    env.set(
        "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
        home.join("no-such-amplihack-hooks"),
    );
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
    if bypass {
        // The bypass only silences the warning. It never lets an older
        // binary stage over a newer install.
        env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    }
    assert_home_is(home);
    // The newer install left its lock file behind. Created through the lock
    // module, so it has the mode and owner the lock checks for.
    drop(install_lock::acquire().expect("creating the install lock file"));
    assert!(install_lock::lock_path(home).is_file());

    let newer = newer_version();
    let installs = Cell::new(0u32);
    let mut checks = 0u32;
    let mut before = None;
    let mut notice = Vec::new();
    let result = ensure_framework_installed_using(
        &mut notice,
        || {
            checks += 1;
            assert_eq!(
                checks, 1,
                "under the lock Guard #2 must find the v{newer} stamp before the assets \
                 are re-checked"
            );
            assert!(
                !install_lock::held_by_current_thread(),
                "the bootstrap's first check must run without the install lock"
            );
            // The newer install finishes now, between the unlocked check and
            // the lock.
            version_stamp::write_installed_version(&newer)?;
            before = Some(snapshot(home));
            Ok(true)
        },
        counting_installer(&installs),
    );

    result.unwrap_or_else(|e| {
        panic!(
            "v{} finding a v{newer} stamp under the lock must stop the bootstrap, \
             not fail the launch: {e:#}",
            crate::VERSION
        )
    });
    assert_eq!(
        installs.get(),
        0,
        "the bootstrap staged over a v{newer} install that finished while it waited \
         for the lock"
    );
    let before = before.expect("the bootstrap never ran its unlocked check");
    assert_eq!(
        changes(&before, &snapshot(home)),
        Vec::<String>::new(),
        "after finding a v{newer} stamp under the lock the bootstrap must change nothing \
         under HOME: no staged assets, no slash commands, no settings.json, no stamp"
    );
    let notice = String::from_utf8_lossy(&notice);
    if bypass {
        assert_eq!(
            notice, "",
            "under AMPLIHACK_SKIP_AUTO_INSTALL the bootstrap stops without a warning"
        );
    } else {
        assert_refusal_warning(&notice, home, &newer);
    }
}

#[test]
fn a_newer_stamp_found_under_the_lock_stops_the_bootstrap() {
    newer_stamp_found_under_the_lock(false);
}

#[test]
fn a_newer_stamp_found_under_the_lock_stops_the_bootstrap_under_the_bypass() {
    newer_stamp_found_under_the_lock(true);
}

/// AC8. A launch whose assets are complete decides that without the lock: it
/// neither waits behind an unrelated install nor creates the lock file.
#[test]
fn a_launch_with_complete_assets_takes_no_lock_and_installs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _env = IsolatedHome::new(home);
    assert_home_is(home);
    create_minimal_staged_assets(home);
    let lock = install_lock::lock_path(home);
    assert!(!lock.exists(), "precondition: no install lock file yet");

    let installs = Cell::new(0u32);
    let mut notice = Vec::new();
    let outcome = restage_framework_if_needed(
        &mut notice,
        framework_restage_needed_now,
        counting_installer(&installs),
    )
    .expect("a complete install needs no re-stage");

    assert_eq!(outcome, LockedRestage::NotNeeded);
    assert_eq!(installs.get(), 0, "complete assets were re-staged");
    assert_eq!(String::from_utf8_lossy(&notice), "");
    assert!(
        !lock.exists(),
        "a launch with complete assets must not take the install lock"
    );
}

/// The shared helper's contract, as self-heal and the bootstrap rely on it:
/// the predicate and the installer both run while the lock is held, the
/// predicate receives the stamp read under the lock, the installer writes to
/// the caller's notice, and the lock is released on return.
#[test]
fn the_shared_helper_rechecks_and_installs_under_the_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _env = IsolatedHome::new(home);
    assert_home_is(home);
    let mut notice: Vec<u8> = Vec::new();

    let outcome = restage_under_install_lock(
        &mut notice,
        |stamp: Option<&str>| {
            assert!(install_lock::held_by_current_thread());
            assert_eq!(stamp, None, "no stamp has been written yet");
            Ok(false)
        },
        |_notice: &mut Vec<u8>| -> anyhow::Result<()> {
            unreachable!("the predicate found nothing to do, so nothing is installed")
        },
    )
    .expect("a re-check that finds nothing to do is not a failure");
    assert_eq!(outcome, LockedRestage::NotNeeded);
    assert!(!install_lock::held_by_current_thread());

    version_stamp::write_installed_version(crate::VERSION).unwrap();
    let outcome = restage_under_install_lock(
        &mut notice,
        |stamp: Option<&str>| {
            assert!(
                install_lock::held_by_current_thread(),
                "the re-check must run under the install lock"
            );
            assert_eq!(
                stamp,
                Some(crate::VERSION),
                "the predicate receives the stamp read under the lock"
            );
            Ok(true)
        },
        |notice: &mut Vec<u8>| -> anyhow::Result<()> {
            assert!(
                install_lock::held_by_current_thread(),
                "the install must run under the install lock"
            );
            notice.extend_from_slice(b"fixture install ran\n");
            Ok(())
        },
    )
    .expect("the fixture install succeeds");
    assert_eq!(outcome, LockedRestage::Installed);
    assert_eq!(
        String::from_utf8_lossy(&notice),
        "fixture install ran\n",
        "the installer writes to the caller's notice, and the helper adds nothing"
    );
    assert!(
        !install_lock::held_by_current_thread(),
        "the lock is released when the helper returns"
    );
}

/// D5. A failed re-stage still fails the launch, as before the fix: the
/// bootstrap does not take over self-heal's kept-bundle handling, even when
/// the installed bundle would pass the compatibility check. The lock is
/// released on the error path.
#[test]
fn a_failed_bootstrap_restage_fails_the_launch_and_releases_the_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let mut env = IsolatedHome::new(home);
    env.set(
        "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
        home.join("no-such-amplihack-hooks"),
    );
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
    assert_home_is(home);
    // A compatible bundle with one framework directory gone: the bootstrap
    // re-stages, and self-heal's kept-bundle rule would accept this bundle.
    create_minimal_staged_assets(home);
    fs::remove_dir_all(home.join(".amplihack/.claude").join(ESSENTIAL_DIRS[0])).unwrap();
    assert!(
        framework_restage_needed_now().unwrap(),
        "precondition: a missing framework directory needs a re-stage"
    );

    let installs = Cell::new(0u32);
    let err =
        ensure_framework_installed_using(&mut Vec::new(), framework_restage_needed_now, || {
            installs.set(installs.get() + 1);
            assert!(install_lock::held_by_current_thread());
            Err(SourceFetchFailed::Failed {
                version: crate::VERSION.to_string(),
                reason: "fixture: network unreachable".to_string(),
            }
            .into())
        })
        .expect_err("a failed bootstrap re-stage must fail the launch");

    assert_eq!(installs.get(), 1);
    assert!(
        err.downcast_ref::<SourceFetchFailed>().is_some(),
        "the install error must reach the caller unchanged, not be replaced by a later \
         step's error; got: {err:#}"
    );
    assert!(
        !install_lock::held_by_current_thread(),
        "the install lock must be released when the re-stage fails"
    );
}
