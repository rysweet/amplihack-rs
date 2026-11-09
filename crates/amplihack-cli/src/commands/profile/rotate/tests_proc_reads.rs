//! Issue #2040: how a rotate cycle reads running sessions.
//!
//! Contract (see "Commands that read afresh" in
//! docs/reference/profile-command.md):
//! - `observe::snapshot(ctx, state, Read)` reads through `Ctx::procs`:
//!   `Read::Cached` may be served from the shared snapshot, `Read::Forced`
//!   always scans, and `Read::Confirmed` forces only when the cached answer
//!   is `Known` (the only answer that permits an action).
//! - AC4: step 6 (revalidation) always reads afresh, even when a fresh
//!   snapshot exists; when the snapshot lock is held at that moment, every
//!   profile counts as running, so a swap ends with
//!   `target eligibility: session running`.
//!
//! The fixture is `tests_revalidate_cause::env3()`. `Ctx::procs` is a
//! `ProcSource` with a synthetic scan, counted per test, a fixed injected
//! clock (so a cached read never goes stale during a test) and a runtime
//! directory inside the fixture's temp dir. No `/proc` is read.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::observe::{Measurement, Read, Snap, decide_measured, snapshot};
use super::tests_revalidate_cause::{ctx_with, env3, log_of, ok, probes_for, spent};
use super::tests_swap::{Env, env, ino_at};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, now};
use crate::commands::profile::proc_snapshot::{
    Collected, ProcRecord, ProcSource, SourceDeps, store,
};
use amplihack_utils::secure_files::euid;
use fs2::FileExt;
use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A process of this user that is not the reader.
const OTHER: u32 = 5151;

fn on(profiles: &[&str]) -> Collected {
    Collected::Procs(
        profiles
            .iter()
            .enumerate()
            .map(|(i, p)| ProcRecord {
                pid: OTHER + i as u32,
                zombie: false,
                claude_like: true,
                profile: Some(p.to_string()),
                environ_unreadable: false,
            })
            .collect(),
    )
}

struct Reads {
    scans: Arc<AtomicUsize>,
    rt: PathBuf,
    proc_root: PathBuf,
}

/// Points `ctx.procs` at a counted synthetic scan whose `i`th answer is
/// `answers[i]` (the last repeats), with the clock fixed at `now()`.
fn counted(e: &Env, ctx: &mut Ctx, answers: Vec<Collected>) -> Reads {
    let base = e._tmp.path().canonicalize().unwrap();
    let rt = base.join("rt");
    let proc_root = base.join("fake-proc");
    for d in [&rt, &proc_root] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let scans = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&scans);
    let t = now();
    ctx.procs = ProcSource::with(SourceDeps {
        proc_root: proc_root.clone(),
        runtime_dir: Some(rt.clone()),
        clock: Arc::new(move || t),
        uid: euid(),
        pid: std::process::id(),
        collect: Arc::new(move |_: &Path, _: &Path, _: u32| {
            let i = c.fetch_add(1, Ordering::SeqCst);
            answers[i.min(answers.len() - 1)].clone()
        }),
    });
    Reads {
        scans,
        rt,
        proc_root,
    }
}

impl Reads {
    fn scans(&self) -> usize {
        self.scans.load(Ordering::SeqCst)
    }

    fn lock_path(&self, root: &Path) -> PathBuf {
        let key = store::key(
            &self.proc_root.canonicalize().unwrap(),
            &root.canonicalize().unwrap(),
        );
        self.rt
            .join("amplihack")
            .join(format!("proc-snapshot-{key}.lock"))
    }
}

fn hold(path: &Path) -> File {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .unwrap();
    f.try_lock_exclusive()
        .expect("the test takes the snapshot lock first");
    f
}

fn never(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("these tests call snapshot() directly")
}

// --- observe::snapshot and Read ------------------------------------------------

#[test]
fn a_cached_snapshot_read_scans_once_within_the_freshness_period() {
    let e = env();
    let mut ctx = ctx_with(&e, never);
    let r = counted(&e, &mut ctx, vec![on(&[])]);
    let state = State::new(&e.root).unwrap();
    assert!(snapshot(&ctx, &state, Read::Cached).unwrap().sees_sessions);
    assert!(snapshot(&ctx, &state, Read::Cached).unwrap().sees_sessions);
    assert_eq!(r.scans(), 1);
}

#[test]
fn a_forced_snapshot_read_scans_even_beside_a_fresh_snapshot() {
    let e = env();
    let mut ctx = ctx_with(&e, never);
    let r = counted(&e, &mut ctx, vec![on(&[])]);
    let state = State::new(&e.root).unwrap();
    snapshot(&ctx, &state, Read::Cached).unwrap();
    snapshot(&ctx, &state, Read::Forced).unwrap();
    snapshot(&ctx, &state, Read::Forced).unwrap();
    assert_eq!(r.scans(), 3);
}

#[test]
fn a_forced_snapshot_read_with_the_lock_held_sees_no_sessions_and_no_candidates() {
    let e = env();
    let mut ctx = ctx_with(&e, never);
    let r = counted(&e, &mut ctx, vec![on(&[])]);
    let state = State::new(&e.root).unwrap();
    let _held = hold(&r.lock_path(&e.root));
    let snap = snapshot(&ctx, &state, Read::Forced).unwrap();
    assert!(!snap.sees_sessions, "Everything is not a known set");
    assert!(snap.candidates.is_empty(), "{:?}", snap.candidates);
    assert_eq!(r.scans(), 0);
}

#[test]
fn a_confirmed_read_forces_only_after_a_known_cached_answer() {
    let e = env();
    let mut ctx = ctx_with(&e, never);
    let r = counted(&e, &mut ctx, vec![on(&[])]);
    let state = State::new(&e.root).unwrap();
    snapshot(&ctx, &state, Read::Cached).unwrap();
    assert_eq!(r.scans(), 1);
    snapshot(&ctx, &state, Read::Confirmed).unwrap();
    assert_eq!(r.scans(), 2, "Known permits, so the read is forced");
}

#[test]
fn a_confirmed_read_does_not_force_after_an_unknown_cached_answer() {
    let e = env();
    let mut ctx = ctx_with(&e, never);
    let r = counted(&e, &mut ctx, vec![Collected::Unsupported]);
    let state = State::new(&e.root).unwrap();
    let snap = snapshot(&ctx, &state, Read::Confirmed).unwrap();
    assert!(!snap.sees_sessions);
    assert_eq!(r.scans(), 1, "only the cached read's own refresh");
}

// --- AC4: step 6 always reads afresh -----------------------------------------

/// Only `spare2` is probed, with room, so it is the only possible target.
fn spare2_has_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare2", ok(0.0, 10.0))]);
    decide_measured(ctx, snap, spent(), probes, now())
}

/// Step 3 scans and writes a fresh snapshot with no session. A session then
/// starts on `spare2`. With the clock fixed, a cached read in step 6 would
/// serve the stale "no session" answer and swap; a forced read sees the
/// session and refuses.
#[cfg(target_os = "linux")]
#[test]
fn ac4_revalidation_scans_afresh_even_when_a_fresh_snapshot_exists() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, spare2_has_room);
    let r = counted(&e, &mut ctx, vec![on(&[]), on(&["spare2"])]);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert!(
        log.contains("target eligibility: session running"),
        "step 6 must see the new session:\n{log}"
    );
    assert!(r.scans() >= 2, "step 6 scanned: {} scans", r.scans());
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert_eq!(
        ino_at(&e.slot("spare2")),
        Some(s2),
        "spare2 was not touched"
    );
}

thread_local! {
    static LOCK_AT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static HELD: RefCell<Option<File>> = const { RefCell::new(None) };
}

/// As [`spare2_has_room`], and takes the snapshot lock while measuring, as
/// a concurrent refresher would, so step 6 finds it held.
fn spare2_has_room_lock_taken(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let path = LOCK_AT.with(|l| l.borrow().clone()).expect("lock path set");
    HELD.with(|h| *h.borrow_mut() = Some(hold(&path)));
    spare2_has_room(ctx, snap)
}

#[cfg(target_os = "linux")]
#[test]
fn ac4_revalidation_with_the_snapshot_lock_held_counts_every_profile_as_running() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, spare2_has_room_lock_taken);
    let r = counted(&e, &mut ctx, vec![on(&[])]);
    LOCK_AT.with(|l| *l.borrow_mut() = Some(r.lock_path(&e.root)));
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    HELD.with(|h| h.borrow_mut().take());
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert!(
        log.contains("target eligibility: session running"),
        "a held lock fails closed:\n{log}"
    );
    assert_eq!(r.scans(), 1, "step 6 found the lock held and did not scan");
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare2")), Some(s2));
}
