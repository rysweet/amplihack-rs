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
use super::tests_switch::assert_refused_seeing;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, now};
use crate::commands::profile::proc_snapshot::{
    Collected, ProcRecord, ProcSource, SourceDeps, record::Snapshot, store,
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
    /// The fixed clock.
    t: i64,
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
        // Synthetic pids: every recorded process is still running.
        may_be_running: Arc::new(|_: &Path, _: u32, _: u32| true),
    });
    Reads {
        scans,
        rt,
        proc_root,
        t,
    }
}

impl Reads {
    fn scans(&self) -> usize {
        self.scans.load(Ordering::SeqCst)
    }

    fn key(&self, root: &Path) -> String {
        store::key(
            &self.proc_root.canonicalize().unwrap(),
            &root.canonicalize().unwrap(),
        )
    }

    fn lock_path(&self, root: &Path) -> PathBuf {
        let key = self.key(root);
        self.rt
            .join("amplihack")
            .join(format!("proc-snapshot-{key}.lock"))
    }

    /// Replaces the snapshot with a fresh one holding `collected`.
    fn plant(&self, root: &Path, collected: &Collected) {
        let key = self.key(root);
        let snap = Snapshot::from_collected(collected, euid(), &key, self.t, OTHER);
        store::Store::open(&self.rt, key)
            .unwrap()
            .write(&snap)
            .unwrap();
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

// --- profile switch reads afresh before it moves a login ----------------------

/// A switch context whose running sessions come from [`counted`].
fn switch_reads(e: &Env, answers: Vec<Collected>) -> (ProcSource, Reads) {
    let s = super::settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut ctx = Ctx::new(e.store.clone(), e.home.clone(), s);
    let r = counted(e, &mut ctx, answers);
    (ctx.procs.clone(), r)
}

/// The prelude's cached read says no session runs in `spare`; one starts
/// before the swap. The forced read sees it and the switch refuses.
#[cfg(target_os = "linux")]
#[test]
fn a_switch_reads_afresh_before_taking_a_login_the_cache_called_free() {
    let e = env();
    let (procs, r) = switch_reads(&e, vec![on(&[]), on(&["spare"])]);
    assert_refused_seeing(
        &e,
        "spare",
        "profile spare is not a rotate candidate (session running): \
         a running process has CLAUDE_CONFIG_DIR set to it",
        Some(&procs),
    );
    assert_eq!(r.scans(), 2, "one cached refresh, one forced read");
}

/// A fresh snapshot says no session runs; the snapshot lock is held when
/// the switch would move the login, so the forced read fails closed.
#[cfg(target_os = "linux")]
#[test]
fn a_switch_with_the_snapshot_lock_held_refuses_without_scanning() {
    let e = env();
    let (procs, r) = switch_reads(&e, vec![on(&[])]);
    let state = State::new(&e.root).unwrap();
    let mut ctx = ctx_with(&e, never);
    ctx.procs = procs.clone();
    // Writes the fresh "no session" snapshot the prelude will be served.
    assert!(snapshot(&ctx, &state, Read::Cached).unwrap().sees_sessions);
    assert_eq!(r.scans(), 1);
    let _held = hold(&r.lock_path(&e.root));
    assert_refused_seeing(
        &e,
        "spare",
        "profile spare is not a rotate candidate (session running): \
         running sessions cannot be seen here, so its login may be in use",
        Some(&procs),
    );
    assert_eq!(r.scans(), 1, "neither read scanned behind the held lock");
}

/// With no session before or after, the switch moves the login and the
/// forced read is the second and last scan.
#[cfg(target_os = "linux")]
#[test]
fn a_switch_with_no_session_moves_after_one_forced_read() {
    let e = env();
    let (procs, r) = switch_reads(&e, vec![on(&[])]);
    super::tests_switch::switch_seeing(&e, "spare", Some(&procs)).unwrap();
    assert_eq!(r.scans(), 2, "one cached refresh, one forced read");
}

// --- AC4: forced, not merely confirmed --------------------------------------

thread_local! {
    static PLANT: RefCell<Option<(Reads, PathBuf)>> = const { RefCell::new(None) };
}

/// Another Claude-like process whose environment cannot be read.
fn unreadable_claude() -> Collected {
    Collected::Procs(vec![ProcRecord {
        pid: OTHER,
        zombie: false,
        claude_like: true,
        profile: None,
        environ_unreadable: true,
    }])
}

/// As [`spare2_has_room`], and replaces the snapshot with a fresh one whose
/// answer is `Everything`, which permits nothing. A confirmed read would be
/// served that answer without scanning; only a forced read scans.
fn spare2_has_room_snapshot_says_everything(
    ctx: &Ctx,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    PLANT.with(|p| {
        let p = p.borrow();
        let (r, root) = p.as_ref().expect("plant set");
        r.plant(root, &unreadable_claude());
    });
    spare2_has_room(ctx, snap)
}

/// AC4 says step 6 always reads afresh. A fresh snapshot that permits
/// nothing would stop a confirmed read from scanning, so the second scan
/// shows the read is forced.
#[cfg(target_os = "linux")]
#[test]
fn ac4_revalidation_scans_even_when_the_cached_answer_permits_nothing() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, spare2_has_room_snapshot_says_everything);
    let r = counted(&e, &mut ctx, vec![on(&[])]);
    let scans = Arc::clone(&r.scans);
    PLANT.with(|p| *p.borrow_mut() = Some((r, e.root.clone())));
    run_cycle(&mut ctx, Mode::Watch).unwrap();
    PLANT.with(|p| p.borrow_mut().take());
    let log = log_of(&e);
    assert_eq!(
        scans.load(Ordering::SeqCst),
        2,
        "step 3 refreshed, step 6 forced:\n{log}"
    );
    assert!(
        !log.contains("target eligibility: session running"),
        "the forced read saw no session:\n{log}"
    );
}
