//! Issue #1658 (C): the on-disk loop guard (`FsGuard`) and resume log of
//! `amplihack-hooks limit-resume`.
//!
//! Layout under `<root>/.auto-switch.d/resume/` (docs/reference/
//! profile-command.md, "Resume state directory"):
//!   lock, events/<sha256 hex>, sessions/<sha256 hex>, resume.log, resume.log.1
//!
//! Security rules pinned here: every directory is created one level at a
//! time with mode 0700, a symlinked, foreign-owned or group/other-writable
//! path is refused and never chmod-ed, and only hashes, counts and epoch
//! seconds reach the disk.

#![cfg(unix)]

#[path = "../../amplihack-cli/tests/support/wait.rs"]
mod wait;

use amplihack_hooks::limit_resume::decide::{event_key, session_key};
use amplihack_hooks::limit_resume::guard::{
    CommitError, FsGuard, GuardStore, LOCK_TIMEOUT, PROMPTED_RETENTION_SECS,
};
use amplihack_hooks::limit_resume::keys::{Key, Pane};
use amplihack_hooks::limit_resume::log::{LOG_MAX_BYTES, LogEntry, append, format_line, sanitize};
use amplihack_hooks::limit_resume::{
    Deps, Event, MAX_RESUMES_PER_SESSION, Outcome, check_watch_dir, record_prompt_at, run_with,
};
use amplihack_types::HookInput;
use amplihack_utils::clock::Clock;
use amplihack_utils::rotator_status::CycleStatus;
use amplihack_utils::rotator_status::swap_record::{
    SwapReading, SwapRecord, read_stop_time, read_swap, rotate_dir,
};
use amplihack_utils::secure_files::{
    ExclusiveLock, lock_exclusive_timeout, open_private_nofollow, write_private_atomic,
};
use serde_json::json;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::time::{Duration, Instant};

const NOW: i64 = 1_792_592_000;
const DAY: i64 = 86_400;

fn real_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().mode() & 0o7777
}

fn private_dir(p: &Path) {
    fs::create_dir(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A profile root as the rotator leaves it: private, with `.auto-switch.d`.
fn root() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    private_dir(&t.path().join(".auto-switch.d"));
    t
}

fn resume_dir(root: &Path) -> PathBuf {
    root.join(".auto-switch.d").join("resume")
}

fn ev(n: u32) -> String {
    event_key(
        "s1",
        Some("rate_limit"),
        None,
        Some(format!("hit your limit {n}").as_str()),
        Some("spare"),
    )
}

fn sess() -> String {
    session_key("s1")
}

fn set_mtime(p: &Path, secs: i64) {
    let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
    let ts = libc::timespec {
        tv_sec: secs as libc::time_t,
        tv_nsec: 0,
    };
    let times = [ts, ts];
    // SAFETY: valid NUL-terminated path and a two-element timespec array.
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    assert_eq!(rc, 0);
}

fn all_bytes_under(dir: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            out.extend_from_slice(e.file_name().to_string_lossy().as_bytes());
            out.push(b'\n');
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                out.extend(fs::read(e.path()).unwrap());
            }
        }
    }
    out
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

// ── FsGuard: layout and basic contract ──────────────────────────────────────

#[test]
fn constructing_a_guard_touches_nothing() {
    let r = root();
    let _g = FsGuard::new(r.path());
    assert!(!resume_dir(r.path()).exists());
}

#[test]
fn commit_creates_private_layout_with_hash_names_only() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    g.commit(&ev(1), &sess(), NOW, NOW).unwrap();

    let d = resume_dir(r.path());
    for sub in [d.clone(), d.join("events"), d.join("sessions")] {
        assert_eq!(mode(&sub), 0o700, "{}", sub.display());
    }
    let marker = d.join("events").join(ev(1));
    let m = fs::symlink_metadata(&marker).unwrap();
    assert!(m.is_file());
    assert_eq!(m.mode() & 0o777, 0o600);
    assert_eq!(m.len(), 0, "markers are empty");

    let session_file = d.join("sessions").join(sess());
    assert_eq!(mode(&session_file), 0o600);
    assert_eq!(
        fs::read_to_string(&session_file).unwrap(),
        format!("{NOW}\n")
    );
}

#[test]
fn commit_creates_auto_switch_d_when_missing() {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut g = FsGuard::new(t.path());
    g.commit(&ev(1), &sess(), NOW, NOW).unwrap();
    assert_eq!(mode(&t.path().join(".auto-switch.d")), 0o700);
}

#[test]
fn seen_and_resumes_reflect_commits() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    assert!(!g.seen(&ev(1)).unwrap());
    assert_eq!(g.resumes(&sess(), NOW).unwrap(), 0);
    g.commit(&ev(1), &sess(), NOW, NOW).unwrap();
    assert!(g.seen(&ev(1)).unwrap());
    assert!(!g.seen(&ev(2)).unwrap());
    assert_eq!(g.resumes(&sess(), NOW).unwrap(), 1);
}

#[test]
fn same_event_twice_is_duplicate() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    g.commit(&ev(1), &sess(), NOW, NOW).unwrap();
    assert!(matches!(
        g.commit(&ev(1), &sess(), NOW + 5, NOW + 5),
        Err(CommitError::Duplicate)
    ));
    assert_eq!(g.resumes(&sess(), NOW + 5).unwrap(), 1);
}

#[test]
fn session_cap_is_three_per_24_hours() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    for i in 0..MAX_RESUMES_PER_SESSION {
        g.commit(&ev(i), &sess(), NOW + i as i64, NOW + i as i64)
            .unwrap();
    }
    assert!(matches!(
        g.commit(&ev(99), &sess(), NOW + 10, NOW + 10),
        Err(CommitError::Cap)
    ));
    assert!(
        !g.seen(&ev(99)).unwrap(),
        "a capped commit leaves no marker"
    );

    // A day later the old resumes no longer count.
    let later = NOW + DAY + 10;
    assert_eq!(g.resumes(&sess(), later).unwrap(), 0);
    g.commit(&ev(100), &sess(), later, later).unwrap();
    let text = fs::read_to_string(resume_dir(r.path()).join("sessions").join(sess())).unwrap();
    assert_eq!(text, format!("{later}\n"), "expired entries are dropped");
}

#[test]
fn sessions_are_capped_independently() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    for i in 0..MAX_RESUMES_PER_SESSION {
        let t = NOW + i as i64;
        g.commit(&ev(i), &sess(), t, t).unwrap();
    }
    g.commit(&ev(50), &session_key("s2"), NOW, NOW).unwrap();
}

#[test]
fn malformed_session_file_counts_as_capped_and_is_left_alone() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    g.commit(&ev(1), &sess(), NOW, NOW).unwrap();
    let f = resume_dir(r.path()).join("sessions").join(sess());
    for junk in [&b"garbage\n"[..], b"123\nabc\n", b"\xff\xfe", b"-1\n"] {
        fs::write(&f, junk).unwrap();
        assert!(
            g.resumes(&sess(), NOW).unwrap() >= MAX_RESUMES_PER_SESSION,
            "{junk:?}"
        );
        assert!(matches!(
            g.commit(&ev(7), &sess(), NOW, NOW),
            Err(CommitError::Cap)
        ));
        assert_eq!(fs::read(&f).unwrap(), junk, "never rewritten");
        assert!(!g.seen(&ev(7)).unwrap());
    }
}

// ── FsGuard: refusing unsafe paths ──────────────────────────────────────────

fn assert_refused(g: &mut FsGuard) {
    assert!(g.seen(&ev(1)).is_err(), "seen");
    assert!(g.resumes(&sess(), NOW).is_err(), "resumes");
    assert!(
        matches!(g.commit(&ev(1), &sess(), NOW, NOW), Err(CommitError::Io)),
        "commit"
    );
}

#[test]
fn symlinked_resume_dir_is_refused() {
    let r = root();
    let elsewhere = r.path().join("elsewhere");
    private_dir(&elsewhere);
    symlink(&elsewhere, resume_dir(r.path())).unwrap();
    assert_refused(&mut FsGuard::new(r.path()));
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn non_private_resume_dir_is_refused_and_not_chmod_ed() {
    for m in [0o755, 0o750, 0o770, 0o707] {
        let r = root();
        let d = resume_dir(r.path());
        fs::create_dir(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(m)).unwrap();
        assert_refused(&mut FsGuard::new(r.path()));
        assert_eq!(mode(&d), m, "mode {m:o} changed");
        assert_eq!(
            fs::read_dir(&d).unwrap().count(),
            0,
            "mode {m:o}: wrote into it"
        );
    }
}

#[test]
fn group_writable_auto_switch_d_is_refused() {
    for m in [0o770, 0o775, 0o777] {
        let r = root();
        let a = r.path().join(".auto-switch.d");
        fs::set_permissions(&a, fs::Permissions::from_mode(m)).unwrap();
        assert_refused(&mut FsGuard::new(r.path()));
        assert_eq!(mode(&a), m);
        assert!(!resume_dir(r.path()).exists());
    }
}

#[test]
fn symlinked_events_or_sessions_dir_is_refused() {
    for sub in ["events", "sessions"] {
        let r = root();
        let d = resume_dir(r.path());
        private_dir(&d);
        let elsewhere = r.path().join("elsewhere");
        private_dir(&elsewhere);
        symlink(&elsewhere, d.join(sub)).unwrap();
        let mut g = FsGuard::new(r.path());
        assert!(
            matches!(g.commit(&ev(1), &sess(), NOW, NOW), Err(CommitError::Io)),
            "{sub}"
        );
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "{sub}");
    }
}

#[test]
fn planted_symlink_marker_is_never_followed() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    private_dir(&d.join("events"));
    private_dir(&d.join("sessions"));
    let target = r.path().join("target");
    symlink(&target, d.join("events").join(ev(1))).unwrap();
    let mut g = FsGuard::new(r.path());
    assert!(g.commit(&ev(1), &sess(), NOW, NOW).is_err());
    assert!(!target.exists(), "create_new must not follow the symlink");
}

#[test]
fn symlinked_session_file_is_never_written_through() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    private_dir(&d.join("events"));
    private_dir(&d.join("sessions"));
    let target = r.path().join("target");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, d.join("sessions").join(sess())).unwrap();
    let mut g = FsGuard::new(r.path());
    assert!(g.commit(&ev(1), &sess(), NOW, NOW).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn symlinked_lock_file_is_refused() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    let target = r.path().join("target");
    symlink(&target, d.join("lock")).unwrap();
    let mut g = FsGuard::new(r.path());
    assert!(matches!(
        g.commit(&ev(1), &sess(), NOW, NOW),
        Err(CommitError::Io)
    ));
    assert!(!target.exists());
}

#[test]
fn malformed_event_key_is_refused() {
    let r = root();
    let mut g = FsGuard::new(r.path());
    let g64 = "g".repeat(64);
    for bad in ["../escape", "ABC", "", g64.as_str()] {
        assert!(g.commit(bad, &sess(), NOW, NOW).is_err(), "{bad:?}");
    }
    assert!(!r.path().join("escape").exists());
}

// ── FsGuard: lock ───────────────────────────────────────────────────────────

/// The resume layout under `root` with its lock held, as a hook still
/// committing leaves it. `events/` and `sessions/` exist already, so a
/// commit reaches the lock without creating them: creating each syncs its
/// parent, and one sync of the test's temporary directory on a loaded disk
/// was measured at 10 s, which counted against the lock-wait bounds.
fn held_lock(root: &Path) -> ExclusiveLock {
    let d = resume_dir(root);
    private_dir(&d);
    private_dir(&d.join("events"));
    private_dir(&d.join("sessions"));
    lock_exclusive_timeout(
        open_private_nofollow(&d.join("lock")).unwrap(),
        Duration::ZERO,
    )
    .unwrap()
}

#[test]
fn stuck_lock_times_out_as_io_error() {
    let r = root();
    let held = held_lock(r.path());
    // Issue #2019: the give-up is checked on virtual time, so a loaded host
    // cannot fail it. The outcome (an Io error and no marker) is the proof.
    let clock = AdvancingClock::new();
    let mut g = FsGuard::with_clock(r.path(), Duration::from_millis(200), clock.clone());
    assert!(matches!(
        g.commit(&ev(1), &sess(), NOW, NOW),
        Err(CommitError::Io)
    ));
    assert!(
        clock.elapsed() >= Duration::from_millis(200),
        "gave up at {:?} of virtual time, before the 200 ms timeout",
        clock.elapsed()
    );
    drop(held);
    assert!(!g.seen(&ev(1)).unwrap(), "no marker after a failed lock");
}

// Issue #2019: no test here measures real elapsed time. The lock wait reads
// an injected clock (`FsGuard::with_clock`); the fakes below keep virtual
// time as a base `Instant` plus an offset.

/// Virtual time that moves only when something sleeps on it, and records
/// the virtual start and length of each sleep. Never sleeps for real.
struct AdvancingClock {
    base: Instant,
    offset_ns: AtomicU64,
    sleeps: Mutex<Vec<(Duration, Duration)>>,
}

impl AdvancingClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            offset_ns: AtomicU64::new(0),
            sleeps: Mutex::new(Vec::new()),
        })
    }

    fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.offset_ns.load(Ordering::SeqCst))
    }
}

impl Clock for AdvancingClock {
    fn now(&self) -> Instant {
        self.base + self.elapsed()
    }

    fn sleep(&self, d: Duration) {
        self.sleeps.lock().unwrap().push((self.elapsed(), d));
        self.offset_ns
            .fetch_add(d.as_nanos() as u64, Ordering::SeqCst);
    }
}

/// Virtual time that never moves, so a lock wait on it never times out
/// however loaded the host is. `sleep` yields the thread instead of
/// sleeping: the spin is bounded by the lock holder's critical section, and
/// no real time is spent (issue #2019, A5).
struct FrozenClock(Instant);

impl FrozenClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Instant::now()))
    }
}

impl Clock for FrozenClock {
    fn now(&self) -> Instant {
        self.0
    }

    fn sleep(&self, _d: Duration) {
        std::thread::yield_now();
    }
}

#[test]
fn default_lock_timeout_is_five_seconds() {
    assert_eq!(LOCK_TIMEOUT, Duration::from_secs(5));
    let r = root();
    assert_eq!(FsGuard::new(r.path()).lock_timeout(), LOCK_TIMEOUT);
    assert_eq!(
        FsGuard::with_lock_timeout(r.path(), Duration::from_millis(7)).lock_timeout(),
        Duration::from_millis(7)
    );

    let held = held_lock(r.path());
    let clock = AdvancingClock::new();
    let mut g = FsGuard::with_clock(r.path(), LOCK_TIMEOUT, clock.clone());
    assert_eq!(g.lock_timeout(), LOCK_TIMEOUT);
    assert!(matches!(
        g.commit(&ev(1), &sess(), NOW, NOW),
        Err(CommitError::Io)
    ));
    let at_failure = clock.elapsed();
    drop(held);
    let sleeps = clock.sleeps.lock().unwrap().clone();
    assert!(!sleeps.is_empty(), "a held lock is waited for");
    let (last_start, _) = *sleeps.last().unwrap();
    assert!(
        last_start < LOCK_TIMEOUT,
        "the last wait starts before 5 s of virtual time: {last_start:?}"
    );
    assert!(
        at_failure >= LOCK_TIMEOUT,
        "the commit gave up at {at_failure:?} of virtual time, before 5 s"
    );
    assert!(
        at_failure < LOCK_TIMEOUT + Duration::from_millis(100),
        "the commit waited {at_failure:?} of virtual time, past one retry beyond 5 s"
    );
    assert!(!g.seen(&ev(1)).unwrap(), "no marker after a failed lock");
}

/// Virtual time that moves only by what is slept on it, and drops the held
/// lock on the first sleep that starts at or after `release_at`: the holder
/// lets go at that point of virtual time, an event the waiter must see.
struct ReleasingClock {
    inner: Arc<AdvancingClock>,
    release_at: Duration,
    held: Mutex<Option<ExclusiveLock>>,
    released_at: Mutex<Option<Duration>>,
}

impl Clock for ReleasingClock {
    fn now(&self) -> Instant {
        self.inner.now()
    }

    fn sleep(&self, d: Duration) {
        self.inner.sleep(d);
        let now = self.inner.elapsed();
        if now >= self.release_at
            && let Some(lock) = self.held.lock().unwrap().take()
        {
            drop(lock);
            *self.released_at.lock().unwrap() = Some(now);
        }
    }
}

/// The default lock wait does not give up before 5 s: a holder that lets go
/// at 4.9 s of virtual time is waited for, and the commit succeeds.
#[test]
fn a_lock_released_at_4_9_virtual_seconds_is_still_taken() {
    let r = root();
    let held = held_lock(r.path());
    let release_at = LOCK_TIMEOUT - Duration::from_millis(100);
    let clock = Arc::new(ReleasingClock {
        inner: AdvancingClock::new(),
        release_at,
        held: Mutex::new(Some(held)),
        released_at: Mutex::new(None),
    });
    let mut g = FsGuard::with_clock(r.path(), LOCK_TIMEOUT, clock.clone());
    let got = g.commit(&ev(1), &sess(), NOW, NOW);
    let released_at = clock
        .released_at
        .lock()
        .unwrap()
        .expect("the holder let go during the wait");
    assert!(released_at < LOCK_TIMEOUT, "released at {released_at:?}");
    assert!(
        got.is_ok(),
        "a lock freed at {released_at:?} of virtual time must be taken: {got:?}"
    );
    assert!(g.seen(&ev(1)).unwrap(), "the commit wrote its marker");
}

/// The same holder letting go only at 5 s is too late: the wait has given up.
#[test]
fn a_lock_released_only_at_5_virtual_seconds_is_given_up_on() {
    let r = root();
    let held = held_lock(r.path());
    let clock = Arc::new(ReleasingClock {
        inner: AdvancingClock::new(),
        release_at: LOCK_TIMEOUT + Duration::from_secs(60),
        held: Mutex::new(Some(held)),
        released_at: Mutex::new(None),
    });
    let mut g = FsGuard::with_clock(r.path(), LOCK_TIMEOUT, clock.clone());
    assert!(matches!(
        g.commit(&ev(1), &sess(), NOW, NOW),
        Err(CommitError::Io)
    ));
    let gave_up = clock.inner.elapsed();
    assert!(
        gave_up >= LOCK_TIMEOUT,
        "gave up at {gave_up:?}, before 5 s"
    );
    assert!(
        clock.released_at.lock().unwrap().is_none(),
        "the wait ended before the holder let go"
    );
    assert!(!g.seen(&ev(1)).unwrap(), "no marker after a failed lock");
}

#[test]
fn a_guard_debug_does_not_show_its_clock() {
    let r = root();
    let g = FsGuard::with_clock(r.path(), LOCK_TIMEOUT, FrozenClock::new());
    let shown = format!("{g:?}");
    assert!(shown.starts_with("FsGuard"), "{shown}");
    assert!(!shown.contains("Frozen"), "{shown}");
    let copy = g.clone();
    assert_eq!(copy.lock_timeout(), LOCK_TIMEOUT);
}

/// Runs `commit` on eight threads released together by a barrier, each
/// with a frozen clock so a loser waiting for the lock cannot time out.
/// Results come back over a channel; a thread that hangs fails the test at
/// the shared `CEILING` instead of hanging it.
fn race_eight(
    root: &Path,
    commit: impl Fn(&mut FsGuard, u32) -> Result<(), CommitError> + Send + Sync + 'static,
) -> Vec<Result<(), CommitError>> {
    let path = Arc::new(root.to_path_buf());
    let barrier = Arc::new(Barrier::new(8));
    let commit = Arc::new(commit);
    let clock = FrozenClock::new();
    let (tx, rx) = mpsc::channel();
    for i in 0..8u32 {
        let (p, b, c, tx) = (path.clone(), barrier.clone(), commit.clone(), tx.clone());
        let clock: Arc<dyn Clock> = clock.clone();
        std::thread::spawn(move || {
            let mut g = FsGuard::with_clock(&p, LOCK_TIMEOUT, clock);
            b.wait();
            let _ = tx.send(c(&mut g, i));
        });
    }
    drop(tx);
    (0..8)
        .map(|n| {
            rx.recv_timeout(wait::CEILING)
                .unwrap_or_else(|e| panic!("racer {n} gave no result: {e}"))
        })
        .collect()
}

#[test]
fn eight_racing_hooks_on_one_event_commit_exactly_once() {
    let r = root();
    let results = race_eight(r.path(), |g, _| g.commit(&ev(1), &sess(), NOW, NOW));
    assert_eq!(results.len(), 8);
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| matches!(r, Err(CommitError::Duplicate))),
        "losers must see Duplicate: {results:?}"
    );
    assert_eq!(FsGuard::new(r.path()).resumes(&sess(), NOW).unwrap(), 1);
}

#[test]
fn eight_racing_waiters_for_one_session_wake_it_once() {
    // Distinct events, one session, all waiting since NOW.
    let r = root();
    let results = race_eight(r.path(), |g, i| g.commit(&ev(i), &sess(), NOW, NOW + 100));
    assert_eq!(results.len(), 8);
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| matches!(r, Err(CommitError::Duplicate))),
        "losers must see Duplicate: {results:?}"
    );
    assert_eq!(
        FsGuard::new(r.path()).resumes(&sess(), NOW + 100).unwrap(),
        1
    );
}

#[test]
fn two_waiters_for_one_session_with_distinct_events_resume_once() {
    // A waits from NOW, B from NOW + 10; whichever commits second loses.
    for b_first in [false, true] {
        let r = root();
        let mut g = FsGuard::new(r.path());
        let (a, b) = ((ev(1), NOW, NOW + 100), (ev(2), NOW + 10, NOW + 105));
        let order = if b_first { [&b, &a] } else { [&a, &b] };
        let first = g.commit(&order[0].0, &sess(), order[0].1, order[0].2);
        let second = g.commit(&order[1].0, &sess(), order[1].1, order[1].2);
        assert!(first.is_ok(), "b_first={b_first}");
        assert!(
            matches!(second, Err(CommitError::Duplicate)),
            "b_first={b_first}: {second:?}"
        );
        assert!(!g.seen(&order[1].0).unwrap(), "the loser leaves no marker");
        assert_eq!(g.resumes(&sess(), NOW + 105).unwrap(), 1);
    }
}

#[test]
fn a_waiter_that_started_in_the_second_of_the_last_resume_may_commit() {
    // A new limit right after a wake, in the same second, is genuine.
    let r = root();
    let mut g = FsGuard::new(r.path());
    g.commit(&ev(1), &sess(), NOW, NOW + 100).unwrap();
    g.commit(&ev(2), &sess(), NOW + 100, NOW + 200).unwrap();
    assert_eq!(g.resumes(&sess(), NOW + 200).unwrap(), 2);
}

// ── FsGuard: retention ──────────────────────────────────────────────────────

#[test]
fn markers_older_than_seven_days_are_pruned_and_nothing_else() {
    let r = root();
    let now = real_now();
    let mut g = FsGuard::new(r.path());
    g.commit(&ev(1), &sess(), now, now).unwrap();
    let events = resume_dir(r.path()).join("events");

    let old = events.join(ev(2));
    fs::write(&old, b"").unwrap();
    fs::set_permissions(&old, fs::Permissions::from_mode(0o600)).unwrap();
    set_mtime(&old, now - 8 * DAY);

    let recent = events.join(ev(3));
    fs::write(&recent, b"").unwrap();
    fs::set_permissions(&recent, fs::Permissions::from_mode(0o600)).unwrap();
    set_mtime(&recent, now - 6 * DAY);

    let odd_name = events.join("not-a-hash");
    fs::write(&odd_name, b"").unwrap();
    set_mtime(&odd_name, now - 30 * DAY);

    let old_link = events.join(ev(4));
    symlink("/nonexistent", &old_link).unwrap();
    set_mtime(&old_link, now - 30 * DAY);

    g.commit(&ev(5), &session_key("s9"), now, now).unwrap();

    assert!(!old.exists(), "8-day-old marker pruned");
    assert!(recent.exists(), "6-day-old marker kept");
    assert!(odd_name.exists(), "non-hash names are left alone");
    assert!(
        fs::symlink_metadata(&old_link).is_ok(),
        "symlinks are left alone"
    );
}

// ── Log format ──────────────────────────────────────────────────────────────

fn entry(outcome: Outcome, live: Option<&str>) -> LogEntry {
    LogEntry {
        session: "3f9a1c2b7e04".into(),
        event: "7d41e0a9c2b1".into(),
        outcome,
        live: live.map(str::to_string),
        cycle: live.map(|_| "swapped".to_string()),
        swap_seq: None,
        swap_live: None,
        pane: None,
        keys_sent: None,
        detail: None,
    }
}

#[test]
fn log_line_matches_the_documented_format() {
    assert_eq!(
        format_line(NOW, &entry(Outcome::Resumed, Some("spare"))),
        "2026-10-21T14:13:20Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=resumed live=spare cycle=swapped\n"
    );
    assert_eq!(
        format_line(NOW, &entry(Outcome::NoCapacityTimeout, None)),
        "2026-10-21T14:13:20Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=no-capacity-timeout live=- cycle=-\n"
    );
}

#[test]
fn log_timestamps_are_utc_iso8601() {
    let t = |s| format_line(s, &entry(Outcome::Cap, None))[..20].to_string();
    assert_eq!(t(0), "1970-01-01T00:00:00Z");
    assert_eq!(t(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(t(NOW - 34), "2026-10-21T14:12:46Z");
}

#[test]
fn log_fields_are_restricted_to_a_safe_alphabet() {
    assert_eq!(sanitize("work.2_a:b-c"), "work.2_a:b-c");
    assert_eq!(sanitize("a\nb c/é"), "a-b-c--");
    assert_eq!(sanitize(""), "-");
    let line = format_line(
        NOW,
        &entry(Outcome::Resumed, Some("x\ny=z outcome=resumed")),
    );
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
    assert!(
        line.ends_with("live=x-y-z-outcome-resumed cycle=swapped\n"),
        "{line:?}"
    );
    let mut e = entry(Outcome::Resumed, None);
    e.cycle = Some("stayed cycle=x\n".into());
    let line = format_line(NOW, &e);
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
    assert!(line.ends_with("cycle=stayed-cycle-x-\n"), "{line:?}");
}

#[test]
fn append_creates_a_private_log_and_appends() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    append(&d, "one\n");
    append(&d, "two\n");
    let log = d.join("resume.log");
    assert_eq!(fs::read_to_string(&log).unwrap(), "one\ntwo\n");
    assert_eq!(mode(&log), 0o600);
}

#[test]
fn append_rotates_past_64_kib() {
    assert_eq!(LOG_MAX_BYTES, 64 * 1024);
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    let log = d.join("resume.log");
    let big = "x".repeat(LOG_MAX_BYTES as usize + 1);
    fs::write(&log, &big).unwrap();
    fs::set_permissions(&log, fs::Permissions::from_mode(0o600)).unwrap();
    append(&d, "new\n");
    assert_eq!(fs::read_to_string(d.join("resume.log.1")).unwrap(), big);
    assert_eq!(fs::read_to_string(&log).unwrap(), "new\n");
}

#[test]
fn append_never_writes_through_a_symlink_and_never_panics() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    let target = r.path().join("target");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, d.join("resume.log")).unwrap();
    append(&d, "line\n");
    assert_eq!(fs::read(&target).unwrap(), b"keep");

    // Missing directory: ignored, nothing created.
    let missing = r.path().join("missing");
    append(&missing, "line\n");
    assert!(!missing.exists());
}

// ── End to end on disk: nothing secret is stored ────────────────────────────

/// The real resume state, swap record and prompt markers under a temporary
/// root. Each `wait_event` is one scripted wake: the rotator writes a swap
/// record 61 s into the wait (`Event::Swap`), or, with `prompt_after`, the
/// user prompts in `s1` first (`Event::Prompted`). No tmux pane.
struct DiskDeps {
    root: PathBuf,
    guard: FsGuard,
    clock: i64,
    start: i64,
    /// The user sends a prompt in `s1` this many seconds into the wait.
    prompt_after: Option<i64>,
    armed: bool,
    swaps: u64,
}

impl DiskDeps {
    fn new(root: &Path, prompt_after: Option<i64>) -> Self {
        Self {
            root: root.to_path_buf(),
            guard: FsGuard::new(root),
            clock: NOW,
            start: NOW,
            prompt_after,
            armed: false,
            swaps: 0,
        }
    }
}

/// What the rotator writes after a swap at `at`, through the same private
/// atomic write it uses.
fn write_swap(root: &Path, seq: u64, at: i64, mode: u32) {
    let dir = rotate_dir(root);
    if !dir.exists() {
        private_dir(&dir);
    }
    let r = SwapRecord {
        from_org: None,
        to_org: None,
        affinity: None,
        seq,
        live_login: "spare".into(),
        from_login: "work".into(),
        swapped_at: at,
        reading: SwapReading {
            session: 12,
            week: 40,
            measured_at: at - 30,
        },
    };
    write_private_atomic(
        &dir,
        "swap.json",
        serde_json::to_string(&r).unwrap().as_bytes(),
    )
    .unwrap();
    fs::set_permissions(dir.join("swap.json"), fs::Permissions::from_mode(mode)).unwrap();
}

impl Deps for DiskDeps {
    fn auto_switch_on(&mut self) -> bool {
        true
    }
    fn now(&mut self) -> i64 {
        self.clock
    }
    fn read_cycle(&mut self, _now: i64) -> Option<CycleStatus> {
        Some(CycleStatus {
            at: Some(self.start - 120),
            measured_at: Some(self.start - 150),
            outcome: Some("stayed".into()),
            alert: false,
            live: Some("work".into()),
            overflow: None,
            platform_live: false,
        })
    }
    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }
    fn log(&mut self, now: i64, entry: &LogEntry) {
        append(&resume_dir(&self.root), &format_line(now, entry));
    }
    fn rotator_down(&mut self, _now: i64) -> bool {
        false
    }
    fn arm(&mut self) -> std::io::Result<()> {
        self.armed = true;
        Ok(())
    }
    fn wait_event(&mut self, deadline: i64) -> Event {
        assert!(self.armed, "wait before arm");
        if let Some(t) = self.prompt_after.take() {
            self.clock = self.start + t;
            record_prompt_at(&self.root, Some("s1"), self.clock);
            return Event::Prompted;
        }
        if self.swaps > 0 {
            self.clock = deadline;
            return Event::Deadline;
        }
        self.swaps += 1;
        self.clock = self.start + 61;
        write_swap(&self.root, self.swaps, self.clock, 0o600);
        Event::Swap
    }
    fn read_swap(&mut self, now: i64) -> Option<SwapRecord> {
        read_swap(&self.root, now)
    }
    fn stop_time(&mut self, _transcript: Option<&Path>, _start: i64) -> Option<i64> {
        None
    }
    fn pane(&mut self) -> Option<Pane> {
        None
    }
    fn send_keys(&mut self, _pane: &Pane, _key: Key) -> std::io::Result<()> {
        panic!("no pane, so no keys")
    }
    fn session_alive_now(&mut self) -> bool {
        true
    }
}

#[test]
fn resume_state_holds_no_message_text_or_session_id() {
    let r = root();
    let mut d = DiskDeps::new(r.path(), None);
    let input: HookInput = serde_json::from_value(json!({
        "hook_event_name": "StopFailure",
        "session_id": "SESSION-SENTINEL-7781",
        "error": "rate_limit",
        "error_details": "DETAILS-SENTINEL-4410 sk-ant-oat01-SECRET",
        "last_assistant_message": "MSG-SENTINEL-9932 You've hit your weekly limit"
    }))
    .unwrap();
    assert_eq!(run_with(&input, &mut d), Outcome::Resumed);

    let bytes = all_bytes_under(&resume_dir(r.path()));
    for s in [
        "SESSION-SENTINEL-7781",
        "DETAILS-SENTINEL-4410",
        "MSG-SENTINEL-9932",
        "sk-ant",
        "SECRET",
        "weekly",
        "rate_limit",
    ] {
        assert!(!contains(&bytes, s), "{s} reached the resume state");
    }
    let log = fs::read_to_string(resume_dir(r.path()).join("resume.log")).unwrap();
    assert!(log.contains("outcome=resumed "), "{log}");
    assert!(log.contains("spare"), "the swap's login is logged: {log}");
    assert!(log.contains(&format!(
        "session={}",
        &session_key("SESSION-SENTINEL-7781")[..12]
    )));
}

// ── Prompt markers (UserPromptSubmit side) ──────────────────────────────────

fn prompted_file(root: &Path, id: &str) -> PathBuf {
    resume_dir(root).join("prompted").join(session_key(id))
}

#[test]
fn prompt_marker_is_written_only_when_the_resume_dir_exists() {
    let r = root();
    record_prompt_at(r.path(), Some("s1"), NOW);
    assert!(
        !resume_dir(r.path()).exists(),
        "resume/ must not be created for a user without the feature"
    );

    // No .auto-switch.d either: nothing is created.
    let bare = tempfile::tempdir().unwrap();
    fs::set_permissions(bare.path(), fs::Permissions::from_mode(0o700)).unwrap();
    record_prompt_at(bare.path(), Some("s1"), NOW);
    assert_eq!(fs::read_dir(bare.path()).unwrap().count(), 0);

    private_dir(&resume_dir(r.path()));
    record_prompt_at(r.path(), Some("s1"), NOW);
    let f = prompted_file(r.path(), "s1");
    assert_eq!(fs::read_to_string(&f).unwrap(), format!("{NOW}\n"));
    assert_eq!(mode(&f), 0o600);
    assert_eq!(mode(f.parent().unwrap()), 0o700);
    let mut g = FsGuard::new(r.path());
    assert_eq!(g.prompted_at(&session_key("s1")).unwrap(), Some(NOW));
    assert_eq!(g.prompted_at(&session_key("s2")).unwrap(), None);

    record_prompt_at(r.path(), Some("s1"), NOW + 30);
    assert_eq!(g.prompted_at(&session_key("s1")).unwrap(), Some(NOW + 30));
}

#[test]
fn prompt_marker_holds_no_session_id() {
    let r = root();
    private_dir(&resume_dir(r.path()));
    record_prompt_at(r.path(), Some("SESSION-SENTINEL-5521"), NOW);
    let bytes = all_bytes_under(&resume_dir(r.path()));
    assert!(!contains(&bytes, "SESSION-SENTINEL-5521"));
    assert!(prompted_file(r.path(), "SESSION-SENTINEL-5521").is_file());
}

#[test]
fn prompt_marker_skips_bad_session_ids_and_unsafe_dirs() {
    let r = root();
    private_dir(&resume_dir(r.path()));
    let long = "x".repeat(257);
    for bad in [None, Some(""), Some(long.as_str())] {
        record_prompt_at(r.path(), bad, NOW);
    }
    assert!(!resume_dir(r.path()).join("prompted").exists());

    // A non-private resume/ is never written into or chmod-ed.
    let r = root();
    let d = resume_dir(r.path());
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    record_prompt_at(r.path(), Some("s1"), NOW);
    assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
    assert_eq!(mode(&d), 0o755);

    // A symlinked prompted/ is refused.
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    let elsewhere = r.path().join("elsewhere");
    private_dir(&elsewhere);
    symlink(&elsewhere, d.join("prompted")).unwrap();
    record_prompt_at(r.path(), Some("s1"), NOW);
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(FsGuard::new(r.path()).prompted_at(&sess()).is_err());
}

#[test]
fn malformed_or_public_prompt_marker_is_an_error() {
    let r = root();
    private_dir(&resume_dir(r.path()));
    record_prompt_at(r.path(), Some("s1"), NOW);
    let f = prompted_file(r.path(), "s1");
    let mut g = FsGuard::new(r.path());
    for junk in [&b"abc\n"[..], b"1\n2\n", b"-5\n"] {
        fs::write(&f, junk).unwrap();
        assert!(g.prompted_at(&sess()).is_err(), "{junk:?}");
    }
    // An unsynced write cut short by a crash leaves an empty file: no prompt.
    fs::write(&f, b"").unwrap();
    assert_eq!(g.prompted_at(&sess()).unwrap(), None);
    fs::write(&f, format!("{NOW}\n")).unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(g.prompted_at(&sess()).is_err());
}

#[test]
fn prompt_after_the_hook_started_ends_the_wait_on_disk() {
    for (prompt, want) in [
        (None, Outcome::Resumed),
        (Some(NOW + 10), Outcome::MovedOn),
        (Some(NOW - 10), Outcome::Resumed),
    ] {
        let r = root();
        private_dir(&resume_dir(r.path()));
        if let Some(t) = prompt {
            record_prompt_at(r.path(), Some("s1"), t);
        }
        let mut d = DiskDeps::new(r.path(), None);
        let input: HookInput = serde_json::from_value(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s1",
            "error": "rate_limit"
        }))
        .unwrap();
        assert_eq!(run_with(&input, &mut d), want, "{prompt:?}");
    }
}

#[test]
fn first_ever_waiter_sees_a_prompt_sent_during_the_wait() {
    // A fresh root: no resume/ until the waiter prepares it.
    let r = root();
    assert!(!resume_dir(r.path()).exists());
    let mut d = DiskDeps::new(r.path(), Some(30));
    let input: HookInput = serde_json::from_value(json!({
        "hook_event_name": "StopFailure",
        "session_id": "s1",
        "error": "rate_limit"
    }))
    .unwrap();
    assert_eq!(run_with(&input, &mut d), Outcome::MovedOn);
    assert_eq!(mode(&resume_dir(r.path()).join("prompted")), 0o700);
    assert!(prompted_file(r.path(), "s1").is_file());
    let log = fs::read_to_string(resume_dir(r.path()).join("resume.log")).unwrap();
    assert!(log.contains("outcome=moved-on"), "{log}");
}

#[test]
fn prepare_creates_the_layout_and_refuses_unsafe_paths() {
    let r = root();
    FsGuard::new(r.path()).prepare(NOW).unwrap();
    assert_eq!(mode(&resume_dir(r.path())), 0o700);
    assert_eq!(mode(&resume_dir(r.path()).join("prompted")), 0o700);

    let r = root();
    let elsewhere = r.path().join("elsewhere");
    private_dir(&elsewhere);
    symlink(&elsewhere, resume_dir(r.path())).unwrap();
    assert!(FsGuard::new(r.path()).prepare(NOW).is_err());
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn old_prompt_markers_are_pruned_on_prepare_and_commit() {
    assert_eq!(PROMPTED_RETENTION_SECS, 7 * 24 * 3600 + 3600);
    for on_commit in [false, true] {
        let r = root();
        private_dir(&resume_dir(r.path()));
        let now = real_now();
        record_prompt_at(r.path(), Some("old"), now);
        record_prompt_at(r.path(), Some("young"), now);
        let old = prompted_file(r.path(), "old");
        let young = prompted_file(r.path(), "young");
        set_mtime(&old, now - PROMPTED_RETENTION_SECS - 10);
        set_mtime(&young, now - PROMPTED_RETENTION_SECS + 600);
        let odd = resume_dir(r.path()).join("prompted").join("not-a-hash");
        fs::write(&odd, b"").unwrap();
        set_mtime(&odd, now - 30 * DAY);

        let mut g = FsGuard::new(r.path());
        if on_commit {
            g.commit(&ev(1), &sess(), now, now).unwrap();
        } else {
            g.prepare(now).unwrap();
        }
        assert!(!old.exists(), "on_commit={on_commit}: old marker kept");
        assert!(
            young.exists(),
            "on_commit={on_commit}: young marker removed"
        );
        assert!(odd.exists(), "non-hash names are left alone");
    }
}

// ── Issue #2085: swap records on disk, read as the hook reads them ──────────

/// Reads the real `swap.json` under a temporary profile root through
/// `swap_record::read_swap`; the root is passed in, so neither HOME nor
/// `AMPLIHACK_CLAUDE_PROFILE_ROOT` is consulted. `wake` writes what the
/// rotator writes for the first wake; later wakes are the deadline.
struct SwapDeps {
    root: PathBuf,
    guard: FsGuard,
    clock: i64,
    wake: fn(&Path),
    woken: bool,
}

impl Deps for SwapDeps {
    fn auto_switch_on(&mut self) -> bool {
        true
    }
    fn now(&mut self) -> i64 {
        self.clock
    }
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus> {
        amplihack_utils::rotator_status::read_cycle(&self.root, now)
    }
    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }
    fn log(&mut self, now: i64, entry: &LogEntry) {
        append(&resume_dir(&self.root), &format_line(now, entry));
    }
    fn rotator_down(&mut self, _now: i64) -> bool {
        false
    }
    fn arm(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn wait_event(&mut self, deadline: i64) -> Event {
        if self.woken {
            self.clock = deadline;
            return Event::Deadline;
        }
        self.woken = true;
        self.clock = NOW + 61;
        (self.wake)(&self.root);
        Event::Swap
    }
    fn read_swap(&mut self, now: i64) -> Option<SwapRecord> {
        read_swap(&self.root, now)
    }
    /// As the hook reads it: the real reader on the payload's path.
    fn stop_time(&mut self, transcript: Option<&Path>, start: i64) -> Option<i64> {
        transcript.and_then(|p| read_stop_time(p, start))
    }
    fn pane(&mut self) -> Option<Pane> {
        None
    }
    fn send_keys(&mut self, _pane: &Pane, _key: Key) -> std::io::Result<()> {
        panic!("no pane, so no keys")
    }
    fn session_alive_now(&mut self) -> bool {
        true
    }
}

fn limit_event() -> HookInput {
    serde_json::from_value(json!({
        "hook_event_name": "StopFailure",
        "session_id": "s2085",
        "error": "rate_limit",
        "last_assistant_message": "You've hit your session limit"
    }))
    .unwrap()
}

fn run_on_disk(r: &tempfile::TempDir, wake: fn(&Path)) -> Outcome {
    let mut d = SwapDeps {
        root: r.path().to_path_buf(),
        guard: FsGuard::new(r.path()),
        clock: NOW,
        wake,
        woken: false,
    };
    run_with(&limit_event(), &mut d)
}

fn no_resume_marker(root: &Path) -> bool {
    let events = resume_dir(root).join("events");
    !events.exists() || fs::read_dir(&events).unwrap().count() == 0
}

#[test]
fn issue_2085_a_private_swap_record_on_disk_resumes() {
    let r = root();
    let out = run_on_disk(&r, |root| write_swap(root, 1, NOW + 61, 0o600));
    assert_eq!(out, Outcome::Resumed);
    assert!(!no_resume_marker(r.path()));
}

#[test]
fn issue_2085_a_swap_record_others_can_write_never_resumes() {
    // S1: a record someone else could have written is not read.
    for mode in [0o620, 0o602, 0o666] {
        let r = root();
        let out = match mode {
            0o620 => run_on_disk(&r, |root| write_swap(root, 1, NOW + 61, 0o620)),
            0o602 => run_on_disk(&r, |root| write_swap(root, 1, NOW + 61, 0o602)),
            _ => run_on_disk(&r, |root| write_swap(root, 1, NOW + 61, 0o666)),
        };
        assert_eq!(out, Outcome::NoCapacityTimeout, "mode {mode:o}");
        assert!(no_resume_marker(r.path()), "mode {mode:o}");
    }
}

#[test]
fn issue_2085_a_swap_record_from_before_the_stop_never_resumes() {
    let r = root();
    let out = run_on_disk(&r, |root| write_swap(root, 1, NOW - 30, 0o600));
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert!(no_resume_marker(r.path()));
    let log = fs::read_to_string(resume_dir(r.path()).join("resume.log")).unwrap();
    assert!(log.contains("outcome=swap-stale"), "{log}");
}

#[test]
fn issue_2085_a_ledger_with_capacity_but_no_swap_record_never_resumes() {
    // Before #2085 a stayed cycle measured after the stop resumed; now only
    // a swap record does.
    let r = root();
    let dir = rotate_dir(r.path());
    private_dir(&dir);
    let p = dir.join("ledger.json");
    fs::write(
        &p,
        format!(
            r#"{{"version":1,"live":"work","last_cycle":{{"at":{a},"measured_at":{m},"outcome":"stayed"}}}}"#,
            a = NOW + 61,
            m = NOW + 31
        ),
    )
    .unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let out = run_on_disk(&r, |_| {});
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert!(no_resume_marker(r.path()));
}

// ── Issue #2236: the stop time read from the transcript on disk ────────────

/// A transcript under `r` whose last entry is at `stop`, with `secret` in
/// a field the hook must never log.
fn write_transcript(r: &Path, stop: i64, mode: u32, secret: &str) -> PathBuf {
    let ts = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let p = r.join("transcript.jsonl");
    let body = format!(
        "{{\"type\":\"user\",\"timestamp\":\"{}\",\"message\":\"{secret}\"}}\n\
         {{\"type\":\"assistant\",\"timestamp\":\"{}\",\"message\":\"{secret}\"}}\n",
        ts(stop - 300),
        ts(stop)
    );
    fs::write(&p, body).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    p
}

/// The hook started at `NOW`, ten seconds after the stop the transcript
/// records, and finds a swap made three seconds after that stop.
fn run_late_start(r: &tempfile::TempDir, transcript_mode: u32) -> Outcome {
    write_swap(r.path(), 1, NOW - 7, 0o600);
    let transcript = write_transcript(r.path(), NOW - 10, transcript_mode, "SECRET-2236-MARKER");
    let mut d = SwapDeps {
        root: r.path().to_path_buf(),
        guard: FsGuard::new(r.path()),
        clock: NOW,
        wake: |_| {},
        woken: false,
    };
    let input: HookInput = serde_json::from_value(json!({
        "hook_event_name": "StopFailure",
        "session_id": "s2236",
        "error": "rate_limit",
        "last_assistant_message": "You've hit your session limit",
        "transcript_path": transcript,
    }))
    .unwrap();
    run_with(&input, &mut d)
}

#[test]
fn issue_2236_a_swap_between_the_transcript_stop_and_a_late_start_resumes() {
    let r = root();
    assert_eq!(run_late_start(&r, 0o600), Outcome::Resumed);
    assert!(!no_resume_marker(r.path()));
    let log = fs::read_to_string(resume_dir(r.path()).join("resume.log")).unwrap();
    assert!(log.contains("outcome=resumed"), "{log}");
    assert!(!log.contains("SECRET-2236-MARKER"), "{log}");
    assert!(!log.contains("transcript"), "{log}");
}

#[test]
fn issue_2236_a_transcript_others_can_write_falls_back_to_the_slack_rule() {
    // A refused transcript gives no stop time: the swap seven seconds
    // before the start is then before the stop, as before #2236.
    for mode in [0o620, 0o602] {
        let r = root();
        assert_eq!(
            run_late_start(&r, mode),
            Outcome::NoCapacityTimeout,
            "mode {mode:o}"
        );
        assert!(no_resume_marker(r.path()), "mode {mode:o}");
        let log = fs::read_to_string(resume_dir(r.path()).join("resume.log")).unwrap();
        assert!(!log.contains("SECRET-2236-MARKER"), "{log}");
    }
}

// ── Issue #2085 (S2): watched directories ───────────────────────────────────

#[test]
fn a_private_directory_can_be_watched() {
    let r = root();
    let dir = r.path().join(".auto-switch.d").join("w");
    private_dir(&dir);
    assert!(check_watch_dir(&dir).is_ok());
}

#[test]
fn a_directory_others_can_write_is_not_watched() {
    for m in [0o720, 0o702, 0o777, 0o1777] {
        let r = root();
        let dir = r.path().join(".auto-switch.d").join("w");
        private_dir(&dir);
        fs::set_permissions(&dir, fs::Permissions::from_mode(m)).unwrap();
        assert!(check_watch_dir(&dir).is_err(), "mode {m:o}");
        assert_eq!(mode(&dir), m, "never chmod-ed");
    }
}

#[test]
fn a_symlink_missing_path_or_file_is_not_watched() {
    let r = root();
    let real = r.path().join(".auto-switch.d").join("real");
    private_dir(&real);
    let link = r.path().join(".auto-switch.d").join("link");
    symlink(&real, &link).unwrap();
    assert!(check_watch_dir(&link).is_err(), "symlink");
    assert!(
        check_watch_dir(&r.path().join(".auto-switch.d").join("missing")).is_err(),
        "missing"
    );
    let file = r.path().join(".auto-switch.d").join("file");
    fs::write(&file, b"").unwrap();
    assert!(check_watch_dir(&file).is_err(), "file");
}

#[test]
fn the_prompted_dir_prepare_makes_can_be_watched() {
    let r = root();
    FsGuard::new(r.path()).prepare(NOW).unwrap();
    assert!(check_watch_dir(&resume_dir(r.path()).join("prompted")).is_ok());
}
