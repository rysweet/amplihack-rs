//! Issue #2040: the running-session snapshot (`ProcSource`).
//!
//! Contract (see "Running-session snapshot" in
//! docs/reference/profile-command.md):
//! - A cached read serves a fresh snapshot (`0 <= now - taken_at < 30`)
//!   without scanning. A stale or missing one is refreshed by whoever takes
//!   the snapshot lock with one non-blocking attempt; when the lock is held,
//!   the stale snapshot is served whatever its age, or `Everything` when
//!   there is no valid one.
//! - A forced read never reads the snapshot file. It takes the lock with one
//!   attempt, scans and writes; when the lock is held it is `Everything`.
//! - `confirmed` is a cached read followed, only when `permits` holds, by a
//!   forced read whose answer is returned.
//! - The reader drops its own pid; `Everything` comes from another pid's
//!   Claude-like record with an unreadable environment.
//! - A bad snapshot is treated as missing; a cache fault never changes the
//!   answer, only the number of scans.
//!
//! Nothing here reads the real `/proc`, the real runtime directory or a real
//! home: every scan is either a synthetic `collect` or the real scanner over
//! a fake `/proc` under a temp dir, and the clock is injected. Scans are
//! counted with one `Arc<AtomicUsize>` per test. No test sleeps or waits on
//! wall-clock time; contention is a real `flock` held on a second
//! descriptor.

use super::{Collected, Fresh, Liveness, ProcRecord, ProcSource, SourceDeps, store};
use crate::commands::profile::name::ProfileName;
use amplihack_utils::secure_files::euid;
use fs2::FileExt;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The injected clock's starting point, in Unix seconds.
const T0: i64 = 1_793_634_000;
/// The reading process's pid.
const READER: u32 = 4242;
/// Another process of the same user.
const OTHER: u32 = 5151;
/// The documented freshness period.
const FRESH_SECS: i64 = 30;
/// The documented snapshot size limit.
const MAX_SNAPSHOT: usize = 4 * 1024 * 1024;

// --- Fixture ---------------------------------------------------------------

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    proc_root: PathBuf,
    root: PathBuf,
    rt: PathBuf,
    now: Arc<AtomicI64>,
    scans: Arc<AtomicUsize>,
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn mode_of(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// Permission bits do not stop root.
fn unprivileged() -> bool {
    euid() != 0
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let proc_root = base.join("proc");
    dir700(&proc_root);
    let root = base.join("profiles");
    for name in ["X", "Y", "Z"] {
        dir700(&root.join(name));
    }
    dir700(&root);
    let rt = base.join("rt");
    dir700(&rt);
    Fx {
        _tmp: tmp,
        base,
        proc_root,
        root,
        rt,
        now: Arc::new(AtomicI64::new(T0)),
        scans: Arc::new(AtomicUsize::new(0)),
    }
}

impl Fx {
    /// Dependencies whose scan is synthetic: the `i`th scan answers
    /// `answers[i]`, and the last answer repeats. Every scan is counted.
    fn deps(&self, pid: u32, answers: Vec<Collected>) -> SourceDeps {
        assert!(!answers.is_empty());
        let now = Arc::clone(&self.now);
        let scans = Arc::clone(&self.scans);
        SourceDeps {
            proc_root: self.proc_root.clone(),
            runtime_dir: Some(self.rt.clone()),
            clock: Arc::new(move || now.load(Ordering::SeqCst)),
            uid: euid(),
            pid,
            collect: Arc::new(move |_proc: &Path, _root: &Path, _uid: u32| {
                let i = scans.fetch_add(1, Ordering::SeqCst);
                answers[i.min(answers.len() - 1)].clone()
            }),
            // Synthetic pids: every recorded process is still running.
            may_be_running: Arc::new(|_: &Path, _: u32, _: u32| true),
        }
    }

    fn source(&self, pid: u32, answers: Vec<Collected>) -> ProcSource {
        ProcSource::with(self.deps(pid, answers))
    }

    /// The real scanner over this fixture's fake `/proc`, counted.
    fn scanner(&self, pid: u32) -> ProcSource {
        let now = Arc::clone(&self.now);
        let mut deps = SourceDeps::new(
            self.proc_root.clone(),
            Some(self.rt.clone()),
            Arc::new(move || now.load(Ordering::SeqCst)),
            euid(),
            pid,
        );
        let inner = Arc::clone(&deps.collect);
        let scans = Arc::clone(&self.scans);
        deps.collect = Arc::new(move |p: &Path, r: &Path, u: u32| {
            scans.fetch_add(1, Ordering::SeqCst);
            inner(p, r, u)
        });
        ProcSource::with(deps)
    }

    fn scans(&self) -> usize {
        self.scans.load(Ordering::SeqCst)
    }

    fn at(&self, t: i64) {
        self.now.store(t, Ordering::SeqCst);
    }

    fn key(&self) -> String {
        store::key(
            &self.proc_root.canonicalize().unwrap(),
            &self.root.canonicalize().unwrap(),
        )
    }

    fn dir(&self) -> PathBuf {
        self.rt.join("amplihack")
    }

    fn snapshot_path(&self) -> PathBuf {
        self.dir()
            .join(format!("proc-snapshot-{}.json", self.key()))
    }

    fn lock_path(&self) -> PathBuf {
        self.dir()
            .join(format!("proc-snapshot-{}.lock", self.key()))
    }

    /// Writes `body` as this root's snapshot, mode 0600, as a writer would.
    fn plant(&self, body: &[u8]) {
        dir700(&self.dir());
        let p = self.snapshot_path();
        let _ = fs::remove_file(&p);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
            .unwrap();
        f.write_all(body).unwrap();
    }

    /// A valid snapshot of this root, taken at `taken_at`, with `procs`.
    fn plant_valid(&self, taken_at: i64, procs: serde_json::Value) {
        self.plant(snap_json(euid(), &self.key(), taken_at, procs).as_bytes());
    }

    /// Holds the snapshot lock on a second descriptor, as another process
    /// refreshing the snapshot would.
    fn hold_lock(&self) -> File {
        dir700(&self.dir());
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.lock_path())
            .unwrap();
        f.try_lock_exclusive()
            .expect("the test takes the snapshot lock first");
        f
    }

    /// Whether a fresh descriptor can take the snapshot lock now.
    fn lock_is_free(&self) -> bool {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.lock_path())
            .unwrap();
        let free = f.try_lock_exclusive().is_ok();
        if free {
            f.unlock().unwrap();
        }
        free
    }

    fn snapshot_json(&self) -> serde_json::Value {
        serde_json::from_slice(&fs::read(self.snapshot_path()).unwrap()).unwrap()
    }
}

fn rec(pid: u32, profile: Option<&str>) -> ProcRecord {
    ProcRecord {
        pid,
        zombie: false,
        claude_like: true,
        profile: profile.map(str::to_string),
        environ_unreadable: false,
    }
}

/// One Claude-like process from `OTHER` on each profile in `names`.
fn live(names: &[&str]) -> Collected {
    Collected::Procs(
        names
            .iter()
            .enumerate()
            .map(|(i, name)| rec(OTHER + i as u32, Some(name)))
            .collect(),
    )
}

fn known(names: &[&str]) -> Liveness {
    Liveness::Known(names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>())
}

/// A snapshot body in the documented format.
fn snap_json(uid: u32, hash: &str, taken_at: i64, procs: serde_json::Value) -> String {
    serde_json::json!({
        "version": 1,
        "uid": uid,
        "root_hash": hash,
        "taken_at": taken_at,
        "writer_pid": OTHER,
        "outcome": "ok",
        "procs": procs,
    })
    .to_string()
}

/// A record in the documented format.
fn proc_json(pid: u32, profile: Option<&str>) -> serde_json::Value {
    let mut v = serde_json::json!({
        "pid": pid, "zombie": false,
        "claude_like": true, "environ_unreadable": false,
    });
    if let Some(p) = profile {
        v["profile"] = serde_json::json!(p);
    }
    v
}

/// "Nothing is live" according to the snapshot.
fn nothing_live() -> serde_json::Value {
    serde_json::json!([proc_json(OTHER, None)])
}

// --- Cached reads -----------------------------------------------------------

#[test]
fn a_cached_read_with_no_snapshot_scans_once_and_writes_a_private_snapshot() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 1);
    assert_eq!(mode_of(&f.dir()), 0o700, "amplihack/ is created 0700");
    assert_eq!(mode_of(&f.snapshot_path()), 0o600);
    assert_eq!(mode_of(&f.lock_path()), 0o600);
    let v = f.snapshot_json();
    assert_eq!(v["version"], 1);
    assert_eq!(v["uid"], euid());
    assert_eq!(v["root_hash"], f.key());
    assert_eq!(v["taken_at"], T0);
    assert_eq!(v["writer_pid"], READER);
    assert_eq!(v["outcome"], "ok");
    assert_eq!(v["procs"][0]["profile"], "X");
}

#[test]
fn a_fresh_snapshot_is_served_without_a_scan() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"]), live(&["Y"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    f.at(T0 + FRESH_SECS - 1);
    assert_eq!(
        s.liveness(&f.root, Fresh::Cached),
        known(&["X"]),
        "29 s old is still fresh"
    );
    assert_eq!(f.scans(), 1);
}

#[test]
fn a_snapshot_thirty_seconds_old_is_stale_and_refreshed() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"]), live(&["Y"])]);
    s.liveness(&f.root, Fresh::Cached);
    f.at(T0 + FRESH_SECS);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["Y"]));
    assert_eq!(f.scans(), 2);
    assert_eq!(f.snapshot_json()["taken_at"], T0 + FRESH_SECS);
}

/// AC1: 80 callers every 2 s for two minutes scan at t = 0, 30, 60, 90 and
/// 120 at most, against 4,880 scans without the snapshot.
#[test]
fn ac1_eighty_callers_every_two_seconds_scan_at_most_five_times_in_two_minutes() {
    let f = fx();
    let sources: Vec<ProcSource> = (0..80)
        .map(|i| f.source(10_000 + i, vec![live(&["X"])]))
        .collect();
    let mut calls = 0;
    for t in (0..=120).step_by(2) {
        f.at(T0 + t);
        for s in &sources {
            assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
            calls += 1;
        }
    }
    assert_eq!(calls, 61 * 80);
    let scans = f.scans();
    assert!((1..=5).contains(&scans), "{scans} scans for {calls} calls");
}

#[test]
fn a_second_source_in_another_process_shares_the_snapshot() {
    let f = fx();
    f.source(READER, vec![live(&["X"])])
        .liveness(&f.root, Fresh::Cached);
    let other = f.source(7777, vec![live(&["Y"])]);
    assert_eq!(other.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 1);
}

// --- AC2: a busy lock never makes a caller wait or scan ---------------------

#[test]
fn ac2_cached_with_a_busy_lock_serves_a_stale_snapshot_whatever_its_age() {
    let f = fx();
    f.plant_valid(T0 - 3600, serde_json::json!([proc_json(OTHER, Some("X"))]));
    let _held = f.hold_lock();
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 0, "a held lock means someone else is refreshing");
}

#[test]
fn ac2_cached_with_a_busy_lock_and_no_snapshot_is_everything() {
    let f = fx();
    let _held = f.hold_lock();
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), Liveness::Everything);
    assert_eq!(f.scans(), 0);
    assert!(
        !f.snapshot_path().exists(),
        "nothing is written without the lock"
    );
}

#[test]
fn ac2_cached_with_a_busy_lock_and_an_invalid_snapshot_is_everything() {
    let f = fx();
    f.plant_valid(T0 + 5, nothing_live()); // from the future: invalid
    let _held = f.hold_lock();
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), Liveness::Everything);
    assert_eq!(f.scans(), 0);
}

#[test]
fn ac2_forced_with_a_busy_lock_is_everything_even_beside_a_fresh_snapshot() {
    let f = fx();
    f.plant_valid(T0, nothing_live());
    let _held = f.hold_lock();
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Forced), Liveness::Everything);
    assert_eq!(f.scans(), 0);
}

// --- Forced reads -----------------------------------------------------------

/// SR3: a forced read never reads the snapshot file, so a planted fresh
/// snapshot that claims nothing is live cannot authorise an action.
#[test]
fn sr3_a_forced_read_ignores_a_fresh_snapshot_that_claims_nothing_is_live() {
    let f = fx();
    f.plant_valid(T0, nothing_live());
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Forced), known(&["X"]));
    assert_eq!(f.scans(), 1);
}

#[test]
fn a_forced_read_writes_its_result_so_the_next_cached_read_does_not_scan() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"]), live(&["Y"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Forced), known(&["X"]));
    f.at(T0 + 1);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 1);
}

#[test]
fn every_forced_read_scans_even_within_the_freshness_period() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"])]);
    s.liveness(&f.root, Fresh::Forced);
    s.liveness(&f.root, Fresh::Forced);
    assert_eq!(f.scans(), 2);
}

#[test]
fn the_snapshot_lock_is_released_after_a_refresh() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"])]);
    s.liveness(&f.root, Fresh::Cached);
    assert!(f.lock_is_free());
    s.liveness(&f.root, Fresh::Forced);
    assert!(f.lock_is_free());
}

/// A forked child keeps a copy of the lock's descriptor until it execs. The
/// copy shares the open file, so dropping the holder must unlock it, not
/// only close its own descriptor.
#[test]
fn dropping_the_snapshot_lock_frees_it_while_a_shared_descriptor_stays_open() {
    let f = fx();
    let st = store::Store::open(&f.rt, f.key()).unwrap();
    let store::Lock::Held(held) = st.try_lock().unwrap() else {
        panic!("a free lock was busy");
    };
    let child_copy = held.share();
    drop(held);
    assert!(f.lock_is_free(), "the shared descriptor kept the lock");
    drop(child_copy);
}

// --- Confirmed reads --------------------------------------------------------

fn x_not_live(l: &Liveness) -> bool {
    !l.is_live(&n("X"))
}

#[test]
fn confirmed_does_not_force_when_the_cached_answer_does_not_permit() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"]), live(&[])]);
    assert_eq!(s.confirmed(&f.root, x_not_live), known(&["X"]));
    assert_eq!(f.scans(), 1, "only the cached read's own refresh");
}

#[test]
fn confirmed_forces_when_the_cached_answer_permits_and_returns_the_forced_answer() {
    let f = fx();
    f.plant_valid(T0, nothing_live());
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.confirmed(&f.root, x_not_live), known(&["X"]));
    assert_eq!(f.scans(), 1, "the forced read scanned");
}

#[test]
fn confirmed_with_a_busy_lock_after_a_permitting_cached_answer_is_everything() {
    let f = fx();
    f.plant_valid(T0, nothing_live());
    let _held = f.hold_lock();
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.confirmed(&f.root, x_not_live), Liveness::Everything);
    assert_eq!(f.scans(), 0);
}

/// SR1/SR2: `Everything` and `Unsupported` never permit an action, so a
/// confirmed read that sees either never forces and never acts.
#[test]
fn everything_and_unsupported_never_permit_an_action() {
    for l in [Liveness::Everything, Liveness::Unsupported] {
        for name in ["X", "Y", "Z"] {
            assert!(l.is_live(&n(name)), "{l:?} counts {name} as live");
        }
        assert!(!x_not_live(&l));
        assert!(!matches!(l, Liveness::Known(_)), "the rotate predicate");
    }
}

// --- Deriving the answer (D3, D4) --------------------------------------------

#[test]
fn the_reader_drops_its_own_pid() {
    let f = fx();
    f.plant_valid(
        T0,
        serde_json::json!([proc_json(READER, Some("X")), proc_json(OTHER, Some("Y"))]),
    );
    let reader = f.source(READER, vec![live(&[])]);
    assert_eq!(reader.liveness(&f.root, Fresh::Cached), known(&["Y"]));
    let someone_else = f.source(7777, vec![live(&[])]);
    assert_eq!(
        someone_else.liveness(&f.root, Fresh::Cached),
        known(&["X", "Y"])
    );
    assert_eq!(f.scans(), 0);
}

#[test]
fn the_writer_records_itself_and_drops_itself_only_when_reading() {
    let f = fx();
    let s = f.source(
        READER,
        vec![Collected::Procs(vec![
            rec(READER, Some("X")),
            rec(OTHER, Some("Y")),
        ])],
    );
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["Y"]));
    let pids: Vec<u64> = f.snapshot_json()["procs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["pid"].as_u64().unwrap())
        .collect();
    assert!(pids.contains(&u64::from(READER)), "{pids:?}");
}

#[test]
fn a_zombie_record_is_not_live() {
    let f = fx();
    let mut z = proc_json(OTHER, Some("X"));
    z["zombie"] = serde_json::json!(true);
    f.plant_valid(T0, serde_json::json!([z]));
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&[]));
}

#[test]
fn another_claude_like_process_with_an_unreadable_environment_is_everything() {
    let f = fx();
    let mut r = proc_json(OTHER, None);
    r["environ_unreadable"] = serde_json::json!(true);
    f.plant_valid(T0, serde_json::json!([r, proc_json(OTHER + 1, Some("X"))]));
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), Liveness::Everything);
}

#[test]
fn the_readers_own_unreadable_environment_does_not_make_everything_live() {
    let f = fx();
    let mut r = proc_json(READER, None);
    r["environ_unreadable"] = serde_json::json!(true);
    f.plant_valid(T0, serde_json::json!([r, proc_json(OTHER, Some("X"))]));
    let s = f.source(READER, vec![live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
}

/// A source whose snapshot reads answer from the file, with `gone` the
/// pids whose processes have exited since; every check is recorded.
fn source_with_gone(f: &Fx, gone: &[u32], checked: &Arc<Mutex<Vec<u32>>>) -> ProcSource {
    let mut d = f.deps(READER, vec![live(&[])]);
    let gone: Vec<u32> = gone.to_vec();
    let checked = Arc::clone(checked);
    d.may_be_running = Arc::new(move |_: &Path, pid: u32, _: u32| {
        checked.lock().unwrap().push(pid);
        !gone.contains(&pid)
    });
    ProcSource::with(d)
}

#[test]
fn a_snapshot_record_whose_process_has_exited_is_not_live() {
    // The session in X ended after the snapshot was taken: its login must
    // be returnable now, not after the freshness period (issue #1569).
    let f = fx();
    f.plant_valid(
        T0,
        serde_json::json!([proc_json(OTHER, Some("X")), proc_json(OTHER + 1, Some("Y"))]),
    );
    let checked = Arc::new(Mutex::new(Vec::new()));
    let s = source_with_gone(&f, &[OTHER], &checked);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["Y"]));
    assert_eq!(f.scans(), 0, "the snapshot still answers without a scan");
}

#[test]
fn an_exited_unreadable_claude_record_no_longer_makes_everything_live() {
    let f = fx();
    let mut r = proc_json(OTHER, None);
    r["environ_unreadable"] = serde_json::json!(true);
    f.plant_valid(T0, serde_json::json!([r, proc_json(OTHER + 1, Some("X"))]));
    let checked = Arc::new(Mutex::new(Vec::new()));
    let s = source_with_gone(&f, &[OTHER], &checked);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
}

#[test]
fn a_stale_snapshot_served_under_a_busy_lock_drops_exited_records_too() {
    let f = fx();
    f.plant_valid(T0, serde_json::json!([proc_json(OTHER, Some("X"))]));
    f.at(T0 + FRESH_SECS * 10);
    let _held = f.hold_lock();
    let checked = Arc::new(Mutex::new(Vec::new()));
    let s = source_with_gone(&f, &[OTHER], &checked);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&[]));
}

#[test]
fn only_records_that_can_change_the_answer_are_checked() {
    let f = fx();
    f.plant_valid(
        T0,
        serde_json::json!([
            proc_json(OTHER, None),
            proc_json(READER, Some("Y")),
            proc_json(OTHER + 1, Some("X")),
        ]),
    );
    let checked = Arc::new(Mutex::new(Vec::new()));
    let s = source_with_gone(&f, &[], &checked);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(*checked.lock().unwrap(), vec![OTHER + 1]);
}

#[test]
fn may_be_running_reads_the_status_of_the_pid() {
    use crate::commands::profile::auto_switch::liveness::may_be_running;
    let f = fx();
    let uid = euid();
    let status = |pid: u32, body: String| {
        let d = f.proc_root.join(pid.to_string());
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("status"), body).unwrap();
    };
    status(
        1,
        format!("Name:\tclaude\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\n"),
    );
    status(
        2,
        format!("Name:\tclaude\nState:\tZ (zombie)\nUid:\t{uid}\t{uid}\n"),
    );
    status(
        3,
        format!("Name:\tclaude\nState:\tS (sleeping)\nUid:\t{}\n", uid + 1),
    );
    status(4, "garbled".to_string());
    assert!(may_be_running(&f.proc_root, 1, uid), "running");
    assert!(!may_be_running(&f.proc_root, 2, uid), "a zombie has ended");
    assert!(
        !may_be_running(&f.proc_root, 3, uid),
        "pid now another user's"
    );
    assert!(
        may_be_running(&f.proc_root, 4, uid),
        "doubt counts as running"
    );
    assert!(
        !may_be_running(&f.proc_root, 5, uid),
        "no /proc entry: exited"
    );
}

#[test]
fn an_unsupported_scan_is_cached_like_any_other_result() {
    let f = fx();
    let s = f.source(READER, vec![Collected::Unsupported, live(&[])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), Liveness::Unsupported);
    assert_eq!(f.snapshot_json()["outcome"], "unsupported");
    f.at(T0 + 10);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), Liveness::Unsupported);
    assert_eq!(f.scans(), 1);
}

#[test]
fn a_root_that_cannot_be_canonicalised_is_unsupported_without_a_scan_or_a_write() {
    let f = fx();
    let s = f.source(READER, vec![live(&["X"])]);
    let missing = f.base.join("no-such-root");
    assert_eq!(s.liveness(&missing, Fresh::Cached), Liveness::Unsupported);
    assert_eq!(s.liveness(&missing, Fresh::Forced), Liveness::Unsupported);
    assert_eq!(f.scans(), 0);
    let written = fs::read_dir(f.dir())
        .map(|d| d.flatten().count())
        .unwrap_or(0);
    assert_eq!(written, 0, "nothing is written for an unusable root");
}

// --- AC6: a bad snapshot is treated as missing --------------------------------

/// The positive control for the rejection table: the unmodified template is
/// accepted, so each rejection below is caused by its one defect.
#[test]
fn ac6_the_valid_template_is_accepted() {
    let f = fx();
    f.plant_valid(T0, nothing_live());
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&[]));
    assert_eq!(f.scans(), 0);
}

/// Plants a fresh snapshot saying nothing is live, damaged by `damage`, and
/// checks that the reader ignores it: it scans once and serves the scan,
/// which says `X` is live.
fn assert_rejected(what: &str, damage: impl FnOnce(&Fx)) {
    let f = fx();
    damage(&f);
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(
        s.liveness(&f.root, Fresh::Cached),
        known(&["X"]),
        "{what}: the snapshot must be ignored"
    );
    assert_eq!(f.scans(), 1, "{what}: one scan");
}

#[test]
fn ac6_a_symlinked_snapshot_is_rejected() {
    assert_rejected("symlink", |f| {
        let real = f.base.join("elsewhere.json");
        fs::write(&real, snap_json(euid(), &f.key(), T0, nothing_live())).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        dir700(&f.dir());
        symlink(&real, f.snapshot_path()).unwrap();
    });
}

#[test]
fn ac6_a_snapshot_that_is_not_a_regular_file_is_rejected() {
    assert_rejected("directory", |f| {
        dir700(&f.snapshot_path());
    });
}

#[test]
fn ac6_a_snapshot_open_to_others_is_rejected() {
    for mode in [0o644, 0o640, 0o604, 0o620] {
        assert_rejected(&format!("mode {mode:o}"), |f| {
            f.plant_valid(T0, nothing_live());
            fs::set_permissions(f.snapshot_path(), fs::Permissions::from_mode(mode)).unwrap();
        });
    }
}

#[test]
fn ac6_a_hard_linked_snapshot_is_rejected() {
    assert_rejected("hard link", |f| {
        f.plant_valid(T0, nothing_live());
        fs::hard_link(f.snapshot_path(), f.base.join("second-name")).unwrap();
    });
}

#[test]
fn ac6_a_snapshot_for_another_uid_is_rejected() {
    assert_rejected("uid", |f| {
        f.plant(snap_json(euid().wrapping_add(1), &f.key(), T0, nothing_live()).as_bytes());
    });
}

#[test]
fn ac6_a_snapshot_for_another_root_is_rejected() {
    assert_rejected("root_hash", |f| {
        f.plant(snap_json(euid(), "0000000000000000", T0, nothing_live()).as_bytes());
    });
}

#[test]
fn ac6_a_snapshot_of_another_version_is_rejected() {
    assert_rejected("version", |f| {
        let mut v: serde_json::Value =
            serde_json::from_str(&snap_json(euid(), &f.key(), T0, nothing_live())).unwrap();
        v["version"] = serde_json::json!(2);
        f.plant(v.to_string().as_bytes());
    });
}

#[test]
fn ac6_a_snapshot_from_the_future_is_rejected() {
    for taken_at in [T0 + 1, T0 + 3600, i64::MAX] {
        assert_rejected(&format!("taken_at {taken_at}"), |f| {
            f.plant_valid(taken_at, nothing_live());
        });
    }
}

/// SR11: `now - taken_at` overflows; checked arithmetic, no panic.
#[test]
fn ac6_a_snapshot_whose_age_overflows_is_rejected() {
    assert_rejected("taken_at i64::MIN", |f| {
        f.plant_valid(i64::MIN, nothing_live());
    });
}

#[test]
fn ac6_an_oversized_snapshot_is_rejected() {
    assert_rejected("over 4 MiB", |f| {
        let mut body = snap_json(euid(), &f.key(), T0, nothing_live()).into_bytes();
        body.resize(MAX_SNAPSHOT + 1, b' ');
        f.plant(&body);
    });
}

#[test]
fn ac6_malformed_json_is_rejected() {
    let bodies: [&[u8]; 6] = [b"{", b"", b"null", b"[]", b"{\"version\":1}", &[0xff, 0xfe]];
    for body in bodies {
        assert_rejected(&format!("{body:?}"), |f| f.plant(body));
    }
}

#[test]
fn ac6_a_wrongly_typed_field_is_rejected() {
    assert_rejected("taken_at as a string", |f| {
        let mut v: serde_json::Value =
            serde_json::from_str(&snap_json(euid(), &f.key(), T0, nothing_live())).unwrap();
        v["taken_at"] = serde_json::json!("now");
        f.plant(v.to_string().as_bytes());
    });
}

/// SR12: one bad profile name rejects the whole snapshot.
#[test]
fn ac6_an_invalid_profile_name_rejects_the_whole_snapshot() {
    for bad in ["../x", "", "a/b", ".", "..", "x\0y"] {
        assert_rejected(&format!("profile {bad:?}"), |f| {
            f.plant_valid(
                T0,
                serde_json::json!([proc_json(OTHER, Some(bad)), proc_json(OTHER + 1, None)]),
            );
        });
    }
}

#[test]
fn unknown_fields_are_ignored_and_a_null_or_absent_profile_loads() {
    let f = fx();
    let mut v: serde_json::Value = serde_json::from_str(&snap_json(
        euid(),
        &f.key(),
        T0,
        serde_json::json!([proc_json(OTHER, None), proc_json(OTHER + 1, Some("Y")),]),
    ))
    .unwrap();
    v["added_later"] = serde_json::json!({"x": 1});
    v["procs"][0]["profile"] = serde_json::Value::Null;
    v["procs"][1]["added_later"] = serde_json::json!(true);
    f.plant(v.to_string().as_bytes());
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["Y"]));
    assert_eq!(f.scans(), 0);
}

// --- Cache faults never change the answer (SR28) ------------------------------

/// Invariant 5: a runtime directory that cannot be written still gives the
/// scan's answer; each reader scans for itself.
#[test]
fn a_read_only_snapshot_directory_still_returns_the_scan_result() {
    if !unprivileged() {
        return;
    }
    let f = fx();
    dir700(&f.dir());
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o500)).unwrap();
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(s.liveness(&f.root, Fresh::Forced), known(&["X"]));
    assert_eq!(
        f.scans(),
        3,
        "no snapshot could be kept, so each read scans"
    );
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn no_runtime_directory_means_no_cache_and_the_scan_result() {
    let f = fx();
    let mut deps = f.deps(READER, vec![live(&["X"])]);
    deps.runtime_dir = None;
    let s = ProcSource::with(deps);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 2);
    assert!(!f.dir().exists());
}

/// SR8: an existing `amplihack/` that fails a check is never repaired, and
/// no snapshot is used.
#[test]
fn an_existing_snapshot_directory_open_to_others_is_not_used_or_repaired() {
    let f = fx();
    dir700(&f.dir());
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 2);
    assert_eq!(mode_of(&f.dir()), 0o755, "never repaired");
    assert!(!f.snapshot_path().exists());
}

#[test]
fn a_symlinked_snapshot_directory_is_not_used() {
    let f = fx();
    let elsewhere = f.base.join("elsewhere");
    dir700(&elsewhere);
    symlink(&elsewhere, f.dir()).unwrap();
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 2);
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn a_lock_file_that_fails_its_checks_is_a_cache_fault_not_a_busy_lock() {
    let f = fx();
    dir700(&f.dir());
    let real = f.base.join("lock-target");
    fs::write(&real, b"").unwrap();
    symlink(&real, f.lock_path()).unwrap();
    let s = f.source(READER, vec![live(&["X"])]);
    assert_eq!(
        s.liveness(&f.root, Fresh::Forced),
        known(&["X"]),
        "a refused lock file is not someone refreshing"
    );
    assert_eq!(f.scans(), 1);
}

// --- AC5: the snapshot holds no paths, command lines or environments ----------

/// A process in the fake `/proc` of `f`, owned by this user.
fn fake_process(f: &Fx, pid: u32, environ: &[&[u8]], cmdline: &[u8], exe: Option<&Path>) {
    let dir = f.proc_root.join(pid.to_string());
    fs::create_dir_all(&dir).unwrap();
    let uid = euid();
    fs::write(
        dir.join("status"),
        format!(
            "Name:\tclaude\nState:\tS (sleeping)\nPPid:\t1\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("stat"),
        format!("{pid} (claude) S 1 {pid} {pid} 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 801122 0 0"),
    )
    .unwrap();
    let mut env = Vec::new();
    for e in environ {
        env.extend_from_slice(e);
        env.push(0);
    }
    fs::write(dir.join("environ"), env).unwrap();
    fs::write(dir.join("cmdline"), cmdline).unwrap();
    if let Some(exe) = exe {
        symlink(exe, dir.join("exe")).unwrap();
    }
    symlink(&f.root, dir.join("cwd")).unwrap();
}

#[test]
fn ac5_the_snapshot_holds_no_environment_path_exe_or_command_line() {
    let mut f = fx();
    // A profile root whose path is itself a sentinel.
    let root = f.base.join("profiles-sentinel-c0ffee");
    dir700(&root.join("X"));
    f.root = root;
    fs::create_dir_all(f.proc_root.join("self")).unwrap();
    fs::write(f.proc_root.join("self/environ"), b"HOME=/h\0").unwrap();
    let config_dir = format!("CLAUDE_CONFIG_DIR={}", f.root.join("X").display());
    let exe = Path::new("/opt/claude-exe-sentinel-51d3/bin/claude");
    fake_process(
        &f,
        100,
        &[
            b"SECRET_SENTINEL=env-value-sentinel-93f1",
            config_dir.as_bytes(),
        ],
        b"claude\0--arg-sentinel-8e4b\0",
        Some(exe),
    );
    let s = f.scanner(READER);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(f.scans(), 1);
    let bytes = fs::read(f.snapshot_path()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for sentinel in [
        "env-value-sentinel-93f1",
        "SECRET_SENTINEL",
        "CLAUDE_CONFIG_DIR",
        "profiles-sentinel-c0ffee",
        "claude-exe-sentinel-51d3",
        "arg-sentinel-8e4b",
        f.base.to_str().unwrap(),
    ] {
        assert!(!text.contains(sentinel), "the snapshot holds {sentinel:?}");
    }
    assert_eq!(f.snapshot_json()["procs"][0]["profile"], "X");
}

// --- SR22: the cache key -----------------------------------------------------

#[test]
fn sr22_the_key_is_sixteen_hex_characters_of_proc_root_and_root() {
    let a = store::key(Path::new("/proc"), Path::new("/home/u/profiles"));
    assert_eq!(a.len(), 16);
    assert!(
        a.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_eq!(
        a,
        store::key(Path::new("/proc"), Path::new("/home/u/profiles"))
    );
    assert_ne!(
        a,
        store::key(Path::new("/tmp/proc"), Path::new("/home/u/profiles"))
    );
    assert_ne!(
        a,
        store::key(Path::new("/proc"), Path::new("/home/u/other"))
    );
    // The zero byte separates the two paths: moving a component across it
    // changes the key.
    assert_ne!(
        store::key(Path::new("/a/b"), Path::new("/c")),
        store::key(Path::new("/a"), Path::new("/b/c"))
    );
}

/// SR13: the raw bytes are hashed, not a lossy string.
#[test]
fn sr13_non_utf8_roots_that_differ_have_different_keys() {
    let a = Path::new(OsStr::from_bytes(b"/r/\xff"));
    let b = Path::new(OsStr::from_bytes(b"/r/\xfe"));
    assert_ne!(
        store::key(Path::new("/proc"), a),
        store::key(Path::new("/proc"), b)
    );
}

#[test]
fn sr22_two_proc_roots_keep_separate_snapshots() {
    let f = fx();
    let other_proc = f.base.join("proc2");
    dir700(&other_proc);
    f.source(READER, vec![live(&["X"])])
        .liveness(&f.root, Fresh::Cached);
    let mut deps = f.deps(READER, vec![live(&["Y"])]);
    deps.proc_root = other_proc;
    assert_eq!(
        ProcSource::with(deps).liveness(&f.root, Fresh::Cached),
        known(&["Y"]),
        "a snapshot of another /proc is never served"
    );
    assert_eq!(f.scans(), 2);
    let names: BTreeSet<String> = fs::read_dir(f.dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn two_profile_roots_keep_separate_snapshots() {
    let f = fx();
    let other_root = f.base.join("profiles2");
    dir700(&other_root.join("X"));
    let s = f.source(READER, vec![live(&["X"]), live(&["Y"])]);
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(s.liveness(&other_root, Fresh::Cached), known(&["Y"]));
    assert_eq!(s.liveness(&f.root, Fresh::Cached), known(&["X"]));
    assert_eq!(
        f.scans(),
        2,
        "alternating roots do not invalidate each other"
    );
}

// --- store::runtime_dir (D1, SR6-SR8) ----------------------------------------

struct Homes {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
}

fn homes() -> Homes {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    dir700(&base);
    let home = base.join("home");
    dir700(&home);
    Homes {
        _tmp: tmp,
        base,
        home,
    }
}

impl Homes {
    fn xdg(&self, mode: u32) -> PathBuf {
        let p = self.base.join(format!("xdg-{mode:o}"));
        dir700(&p);
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    fn amplihack(&self, mode: u32) -> PathBuf {
        let p = self.home.join(".amplihack");
        dir700(&p);
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    fn fallback(&self) -> PathBuf {
        self.home.join(".amplihack").join("runtime")
    }
}

#[test]
fn a_private_xdg_runtime_dir_is_used() {
    let h = homes();
    let xdg = h.xdg(0o700);
    assert_eq!(
        store::runtime_dir(Some(xdg.as_os_str()), &h.home, euid()),
        Some(xdg)
    );
    assert!(!h.fallback().exists());
}

#[test]
fn an_unusable_xdg_runtime_dir_falls_back_to_the_amplihack_runtime_dir() {
    let h = homes();
    let real = h.xdg(0o700);
    let link = h.base.join("xdg-link");
    symlink(&real, &link).unwrap();
    let cases: Vec<(&str, Option<PathBuf>)> = vec![
        ("unset", None),
        ("mode 0750", Some(h.xdg(0o750))),
        ("mode 0701", Some(h.xdg(0o701))),
        ("symlink", Some(link)),
        ("relative", Some(PathBuf::from("run/user/1000"))),
        ("missing", Some(h.base.join("no-such-dir"))),
        ("empty", Some(PathBuf::new())),
    ];
    h.amplihack(0o700);
    for (what, xdg) in cases {
        let got = store::runtime_dir(xdg.as_deref().map(Path::as_os_str), &h.home, euid());
        assert_eq!(got, Some(h.fallback()), "{what}");
        assert_eq!(mode_of(&h.fallback()), 0o700, "{what}: created 0700");
    }
}

#[test]
fn the_fallback_is_never_used_without_an_existing_private_amplihack_dir() {
    let h = homes();
    assert_eq!(store::runtime_dir(None, &h.home, euid()), None);
    assert!(
        !h.home.join(".amplihack").exists(),
        "~/.amplihack is never created for this"
    );
    for mode in [0o770, 0o702, 0o777] {
        let a = h.amplihack(mode);
        assert_eq!(
            store::runtime_dir(None, &h.home, euid()),
            None,
            "~/.amplihack mode {mode:o}"
        );
        assert!(!a.join("runtime").exists());
    }
}

#[test]
fn an_existing_fallback_that_fails_a_check_is_not_used_or_repaired() {
    let h = homes();
    h.amplihack(0o700);
    dir700(&h.fallback());
    fs::set_permissions(h.fallback(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(store::runtime_dir(None, &h.home, euid()), None);
    assert_eq!(mode_of(&h.fallback()), 0o755, "never repaired");

    fs::remove_dir(h.fallback()).unwrap();
    let target = h.base.join("runtime-target");
    dir700(&target);
    symlink(&target, h.fallback()).unwrap();
    assert_eq!(store::runtime_dir(None, &h.home, euid()), None, "symlink");
}

#[test]
fn a_directory_owned_by_another_uid_is_never_used() {
    let h = homes();
    let xdg = h.xdg(0o700);
    h.amplihack(0o700);
    let other = euid().wrapping_add(1);
    assert_eq!(
        store::runtime_dir(Some(xdg.as_os_str()), &h.home, other),
        None
    );
}

// --- SR15: the snapshot lock is not inherited ----------------------------------

/// A child started while the snapshot lock is held (here, from inside the
/// scan) must not keep the lock after the parent releases it: the lock file
/// is opened with `O_CLOEXEC`. The child is `cat` on a pipe; it ends when the
/// test closes the pipe, so no signal is sent and nothing is timed.
#[test]
fn sr15_a_child_started_during_a_refresh_does_not_keep_the_snapshot_lock() {
    let f = fx();
    let child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    let mut deps = f.deps(READER, vec![live(&["X"])]);
    let inner = Arc::clone(&deps.collect);
    let slot = Arc::clone(&child);
    deps.collect = Arc::new(move |p: &Path, r: &Path, u: u32| {
        let c = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        *slot.lock().unwrap() = Some(c);
        inner(p, r, u)
    });
    ProcSource::with(deps).liveness(&f.root, Fresh::Forced);
    let free = f.lock_is_free();
    let mut c = child.lock().unwrap().take().expect("the scan ran");
    drop(c.stdin.take());
    c.wait().unwrap();
    assert!(free, "the child inherited the snapshot lock");
}

// --- from_fn: the seam the rotate tests use ----------------------------------

#[test]
fn a_from_fn_source_asks_its_function_for_every_read() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let s = ProcSource::from_fn(move |_root: &Path| {
        c.fetch_add(1, Ordering::SeqCst);
        known(&[])
    });
    let root = Path::new("/nowhere");
    assert_eq!(s.liveness(root, Fresh::Cached), known(&[]));
    assert_eq!(s.liveness(root, Fresh::Forced), known(&[]));
    let copy = s.clone();
    assert_eq!(copy.confirmed(root, x_not_live), known(&[]));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "cached, forced, and both reads of confirmed"
    );
}
