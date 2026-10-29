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

use amplihack_hooks::limit_resume::decide::{event_key, session_key};
use amplihack_hooks::limit_resume::guard::{
    CommitError, FsGuard, GuardStore, PROMPTED_RETENTION_SECS,
};
use amplihack_hooks::limit_resume::log::{LOG_MAX_BYTES, LogEntry, append, format_line, sanitize};
use amplihack_hooks::limit_resume::{
    Deps, MAX_RESUMES_PER_SESSION, Outcome, record_prompt_at, run_with,
};
use amplihack_types::HookInput;
use amplihack_utils::rotator_status::CycleStatus;
use amplihack_utils::secure_files::{lock_exclusive_timeout, open_private_nofollow};
use serde_json::json;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
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

#[test]
fn stuck_lock_times_out_as_io_error() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    let held = lock_exclusive_timeout(
        open_private_nofollow(&d.join("lock")).unwrap(),
        Duration::ZERO,
    )
    .unwrap();
    let mut g = FsGuard::with_lock_timeout(r.path(), Duration::from_millis(200));
    let start = Instant::now();
    assert!(matches!(
        g.commit(&ev(1), &sess(), NOW, NOW),
        Err(CommitError::Io)
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    drop(held);
    assert!(!g.seen(&ev(1)).unwrap(), "no marker after a failed lock");
}

#[test]
fn default_lock_timeout_is_five_seconds() {
    let r = root();
    let d = resume_dir(r.path());
    private_dir(&d);
    let held = lock_exclusive_timeout(
        open_private_nofollow(&d.join("lock")).unwrap(),
        Duration::ZERO,
    )
    .unwrap();
    let mut g = FsGuard::new(r.path());
    let start = Instant::now();
    assert!(g.commit(&ev(1), &sess(), NOW, NOW).is_err());
    let waited = start.elapsed();
    drop(held);
    assert!(
        waited >= Duration::from_millis(4500) && waited < Duration::from_secs(8),
        "waited {waited:?}"
    );
}

// ── FsGuard: concurrency ────────────────────────────────────────────────────

#[test]
fn eight_racing_hooks_on_one_event_commit_exactly_once() {
    let r = root();
    let path = Arc::new(r.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (p, b) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let mut g = FsGuard::new(&p);
                b.wait();
                g.commit(&ev(1), &sess(), NOW, NOW)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| matches!(r, Err(CommitError::Duplicate))),
        "losers must see Duplicate"
    );
    assert_eq!(FsGuard::new(r.path()).resumes(&sess(), NOW).unwrap(), 1);
}

#[test]
fn eight_racing_waiters_for_one_session_wake_it_once() {
    // Distinct events, one session, all waiting since NOW.
    let r = root();
    let path = Arc::new(r.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8u32)
        .map(|i| {
            let (p, b) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let mut g = FsGuard::new(&p);
                b.wait();
                g.commit(&ev(i), &sess(), NOW, NOW + 100)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| matches!(r, Err(CommitError::Duplicate)))
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

struct DiskDeps {
    root: PathBuf,
    guard: FsGuard,
    clock: i64,
    start: i64,
    /// The user sends a prompt in `s1` this many seconds into the wait.
    prompt_after: Option<i64>,
}

impl Deps for DiskDeps {
    fn auto_switch_on(&mut self) -> bool {
        true
    }
    fn now(&mut self) -> i64 {
        self.clock
    }
    fn sleep(&mut self, d: Duration) {
        self.clock += d.as_secs() as i64;
        if self
            .prompt_after
            .is_some_and(|t| self.clock - self.start >= t)
        {
            record_prompt_at(&self.root, Some("s1"), self.clock);
        }
    }
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus> {
        // The rotator records its cycle 61 s into the wait.
        if now < self.start + 61 {
            return None;
        }
        Some(CycleStatus {
            at: Some(self.start + 61),
            measured_at: Some(self.start + 31),
            outcome: Some("swapped".into()),
            alert: false,
            live: Some("spare".into()),
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
    fn session_alive(&mut self) -> bool {
        true
    }
    fn rotator_down(&mut self, _now: i64) -> bool {
        false
    }
}

#[test]
fn resume_state_holds_no_message_text_or_session_id() {
    let r = root();
    let mut d = DiskDeps {
        root: r.path().to_path_buf(),
        guard: FsGuard::new(r.path()),
        clock: NOW,
        start: NOW,
        prompt_after: None,
    };
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
    assert!(
        log.contains("outcome=resumed live=spare cycle=swapped"),
        "{log}"
    );
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
        let mut d = DiskDeps {
            root: r.path().to_path_buf(),
            guard: FsGuard::new(r.path()),
            clock: NOW,
            start: NOW,
            prompt_after: None,
        };
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
    let mut d = DiskDeps {
        root: r.path().to_path_buf(),
        guard: FsGuard::new(r.path()),
        clock: NOW,
        start: NOW,
        prompt_after: Some(30),
    };
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
