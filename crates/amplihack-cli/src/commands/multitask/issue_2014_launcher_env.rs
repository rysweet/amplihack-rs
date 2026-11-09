//! Issue #2014: the multitask launcher tests failed intermittently in the full
//! `--lib` run, because another test set `AMPLIHACK_CLAUDE_PROFILE_ROOT` to
//! invalid UTF-8 while they read the process environment without the shared
//! env lock.
//!
//! The `_with` launchers take the profile variables only from `get`. The
//! two-argument launchers still read the process environment, so a parent's
//! pinned profile reaches nested sessions.
#![cfg(unix)]

use super::command_builder_tests::{keep, no_env};
use super::launcher::{
    write_classic_launcher, write_classic_launcher_with, write_recipe_launcher,
    write_recipe_launcher_with,
};
use super::launcher_tests::{assert_script_is_lf_only_and_bash_valid, test_workstream};
use super::models::Workstream;
use crate::commands::profile::{PROFILE_ENV, ROOT_ENV};
use crate::test_support::with_env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use tempfile::TempDir;

const DELEGATE: &str = "amplihack copilot";

fn non_utf8_root() -> &'static OsStr {
    OsStr::from_bytes(b"/\xff")
}

/// A workstream whose base and state directories are fresh tempdirs, with the
/// work directory created. The tempdirs are returned so that they outlive it.
fn fresh_workstream() -> (TempDir, TempDir, Workstream) {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = test_workstream(base.path(), state.path());
    fs::create_dir_all(&ws.work_dir).unwrap();
    (base, state, ws)
}

fn assert_refused_for_non_utf8_root(result: anyhow::Result<()>, launcher: &str) {
    let msg = format!(
        "{:#}",
        result.expect_err(&format!(
            "{launcher} launcher must read the process environment and refuse a non-UTF-8 root"
        ))
    );
    assert!(msg.contains(ROOT_ENV), "{launcher}: {msg}");
    assert!(msg.contains("not valid UTF-8"), "{launcher}: {msg}");
    assert!(
        !msg.contains('\u{fffd}'),
        "{launcher}: value must not be printed: {msg}"
    );
}

fn build_and_check_recipe(ws: &Workstream) {
    write_recipe_launcher_with(ws, DELEGATE, no_env, keep)
        .expect("recipe launcher with an injected environment must not read the process");
    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("launcher.sh"));
    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
}

fn build_and_check_classic(ws: &Workstream) {
    write_classic_launcher_with(ws, DELEGATE, no_env, keep)
        .expect("classic launcher with an injected environment must not read the process");
    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
}

/// Shows the race deterministically. While the process holds a non-UTF-8
/// root, the two-argument launchers fail with the error from #2014 and write
/// nothing, and the `_with` launchers called with an empty environment succeed.
#[test]
fn process_reading_launchers_see_a_non_utf8_root_and_the_seam_does_not() {
    let (_rb, _rs, plain_recipe) = fresh_workstream();
    let (_cb, _cs, plain_classic) = fresh_workstream();
    let (_sb, _ss, seam_recipe) = fresh_workstream();
    let (_tb, _ts, seam_classic) = fresh_workstream();

    // The profile name is unset so that the real substitution path, and with
    // it the developer's profile store, is never reached.
    let (plain, seam) = with_env(
        &[(PROFILE_ENV, None), (ROOT_ENV, Some(non_utf8_root()))],
        || {
            let plain = (
                write_recipe_launcher(&plain_recipe, DELEGATE),
                write_classic_launcher(&plain_classic, DELEGATE),
            );
            let seam = (
                write_recipe_launcher_with(&seam_recipe, DELEGATE, no_env, keep),
                write_classic_launcher_with(&seam_classic, DELEGATE, no_env, keep),
            );
            (plain, seam)
        },
    );

    assert_refused_for_non_utf8_root(plain.0, "recipe");
    assert_refused_for_non_utf8_root(plain.1, "classic");

    // Scripts are built before any file is written, so a refused export
    // leaves nothing behind.
    for name in ["run.sh", "launcher.sh", "context.json"] {
        assert!(
            !plain_recipe.work_dir.join(name).exists(),
            "refused recipe launcher left {name} behind"
        );
    }
    for name in ["run.sh", "TASK.md"] {
        assert!(
            !plain_classic.work_dir.join(name).exists(),
            "refused classic launcher left {name} behind"
        );
    }

    seam.0
        .expect("recipe launcher with an injected environment must ignore the process root");
    seam.1
        .expect("classic launcher with an injected environment must ignore the process root");
    assert_script_is_lf_only_and_bash_valid(&seam_recipe.work_dir.join("launcher.sh"));
    assert_script_is_lf_only_and_bash_valid(&seam_recipe.work_dir.join("run.sh"));
    assert_script_is_lf_only_and_bash_valid(&seam_classic.work_dir.join("run.sh"));
}

/// The injected getter and substitute reach the written `run.sh`, so the seam
/// carries the profile and does not just drop it.
#[test]
fn injected_profile_reaches_the_written_run_scripts() {
    let (_rb, _rs, recipe) = fresh_workstream();
    let (_cb, _cs, classic) = fresh_workstream();
    let get = |k: &str| match k {
        k if k == PROFILE_ENV => Some(OsString::from("work")),
        k if k == ROOT_ENV => Some(OsString::from("/p/it's")),
        _ => None,
    };
    let spare = |inherited: &str| (inherited == "work").then(|| "spare".to_string());

    write_recipe_launcher_with(&recipe, DELEGATE, get, spare).unwrap();
    write_classic_launcher_with(&classic, DELEGATE, get, spare).unwrap();

    for ws in [&recipe, &classic] {
        let run_sh = ws.work_dir.join("run.sh");
        let script = fs::read_to_string(&run_sh).unwrap();
        assert!(
            script.contains("export AMPLIHACK_CLAUDE_PROFILE='spare'\n"),
            "{script}"
        );
        assert!(
            script.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/p/it'\\''s'\n"),
            "{script}"
        );
        assert!(!script.contains("'work'"), "{script}");
        assert_script_is_lf_only_and_bash_valid(&run_sh);
    }
}

/// Holds the mutator's window open until it is dropped or finished. Dropping
/// `done` makes the mutator's `recv()` return, so its `with_env` window closes
/// and the join completes, also when the test panics.
struct StopGuard {
    done: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl StopGuard {
    /// Closes the window and joins the mutator. If the mutator panicked,
    /// re-raises its panic so the test shows the original message.
    fn finish(mut self) {
        drop(self.done.take());
        let handle = self.handle.take().expect("mutator already joined");
        if let Err(payload) = handle.join() {
            std::panic::resume_unwind(payload);
        }
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        drop(self.done.take());
        if let Some(handle) = self.handle.take() {
            // Only reached on panic paths; `finish()` checks the join on the
            // normal path. A second panic inside Drop would abort the process.
            let _ = handle.join();
        }
    }
}

/// Starts a thread that opens one `with_env` window with the root set to
/// invalid UTF-8, sends on `ready` once the value is in the process, and
/// keeps the window open until `done` is sent or dropped. `with_env` restores
/// the previous value when the window closes.
fn spawn_mutator() -> (StopGuard, Receiver<()>) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        with_env(&[(ROOT_ENV, Some(non_utf8_root()))], || {
            ready_tx
                .send(())
                .expect("main thread stopped waiting for the mutator");
            // Returns on send or when the guard drops the sender.
            let _ = done_rx.recv();
        });
    });
    let guard = StopGuard {
        done: Some(done_tx),
        handle: Some(handle),
    };
    (guard, ready_rx)
}

/// The regression test for #2014. A parallel thread sets the root to invalid
/// UTF-8 and holds it there while the main thread runs the launcher checks
/// without the env lock. The checks must still pass.
///
/// The overlap is guaranteed by the channel handshake, not by timing: the
/// checks run only after the mutator reports that the invalid value is set,
/// and the mutator restores the value only after the checks finish.
#[test]
fn launcher_tests_pass_while_another_thread_sets_a_non_utf8_root() {
    let (guard, ready) = spawn_mutator();
    ready
        .recv()
        .expect("mutator exited before it set the invalid root");

    // The invalid value is in the process for the whole of the checks. The
    // mutator holds the env lock, so nothing else can change it meanwhile.
    assert_eq!(
        std::env::var_os(ROOT_ENV).as_deref(),
        Some(non_utf8_root()),
        "the mutator's window must be open while the checks run"
    );

    let (_rb, _rs, recipe) = fresh_workstream();
    build_and_check_recipe(&recipe);
    let (_cb, _cs, classic) = fresh_workstream();
    build_and_check_classic(&classic);

    guard.finish();
}
