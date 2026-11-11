//! Issues #1658 and #2085: decision logic of `amplihack-hooks limit-resume`,
//! the `StopFailure` hook that resumes a session stopped by a usage limit
//! once the rotator has swapped to a login with room.
//!
//! Since #2085 the hook does not poll. It snapshots `swap.json`, arms its
//! watches, checks the record once, then blocks in `Deps::wait_event` until
//! the rotator writes a new swap record, the user prompts, the session
//! exits, a watch is lost or the lifetime cap (`MAX_WAIT`) passes. Here the
//! events are scripted: no real waiting, no files, no clock windows.
//! Filesystem behaviour is in `limit_resume_fs.rs`.
//!
//! Resume paths (D7): with a verified tmux pane the hook commits, sends
//! Escape, C-u, `continue` (literal) and Enter, and exits 0
//! (`resumed-keys`); without one it commits and exits 2 with `RESUME_MSG`
//! (`resumed`). Never both. A key that fails after the commit is
//! `resume-keys-failed` and stops the rest; it never falls back to exit 2.
//!
//! Required tests from the issues:
//! (a) a swap record newer than the stop, with room → resume once
//! (b) a stale, unchanged or full record → no resume
//! (c) repeated distinct limit events → bounded by the per-session cap
//! (d) non-limit error → no action
//! (e) auto-switch off → exit 0, no output, nothing read
//! loop: 50 invocations on one event → at most 1 resume
//! moved-on: the user sent a prompt while waiting → no resume
//! one wake per session: concurrent waiters with distinct events → 1 resume

use amplihack_hooks::limit_resume::decide::{
    Facts, Gate, event_key, gate, is_limit, session_id_ok, session_key,
};
use amplihack_hooks::limit_resume::guard::{CommitError, GuardStore};
use amplihack_hooks::limit_resume::keys::{Key, Pane, RESUME_KEYS};
use amplihack_hooks::limit_resume::log::LogEntry;
use amplihack_hooks::limit_resume::{
    Deps, Event, MAX_RESUMES_PER_SESSION, Outcome, RESUME_MSG, run_with, run_with_io, send_resume,
};
use amplihack_types::HookInput;
use amplihack_utils::rotator_status::swap_record::{SwapReading, SwapRecord};
use amplihack_utils::rotator_status::{CycleStatus, MAX_WAIT};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};

const START: i64 = 1_792_592_000;
const DEADLINE: i64 = START + MAX_WAIT.as_secs() as i64;
/// Seconds the fake clock moves for each scripted event.
const STEP: i64 = 60;
const TMUX: &str = "/tmp/tmux-1000/default,4242,0";

// ── Test doubles ────────────────────────────────────────────────────────────

/// In-memory guard with the same contract as `FsGuard`.
#[derive(Default)]
struct MemGuard {
    events: HashSet<String>,
    sessions: HashMap<String, Vec<i64>>,
    /// Last prompt per session, as an absolute time.
    prompted: HashMap<String, i64>,
    fail_seen: bool,
    fail_prompted: bool,
    fail_prepare: bool,
    prepared: u32,
    fail_resumes: bool,
    force_commit: Option<CommitError>,
    calls: u32,
    /// What happened, in order: `commit`, `key:<Key>`, `arm`, `wait`.
    order: Vec<String>,
}

impl GuardStore for MemGuard {
    fn seen(&mut self, event: &str) -> io::Result<bool> {
        self.calls += 1;
        if self.fail_seen {
            return Err(io::Error::other("injected"));
        }
        Ok(self.events.contains(event))
    }

    fn resumes(&mut self, session: &str, now: i64) -> io::Result<u32> {
        self.calls += 1;
        if self.fail_resumes {
            return Err(io::Error::other("injected"));
        }
        Ok(self.sessions.get(session).map_or(0, |v| {
            v.iter().filter(|t| now - **t < 86_400).count() as u32
        }))
    }

    fn last_resume(&mut self, session: &str) -> io::Result<Option<i64>> {
        if self.fail_resumes {
            return Err(io::Error::other("injected"));
        }
        Ok(self
            .sessions
            .get(session)
            .and_then(|v| v.iter().copied().max()))
    }

    fn prepare(&mut self, _now: i64) -> io::Result<()> {
        self.prepared += 1;
        if self.fail_prepare {
            return Err(io::Error::other("injected"));
        }
        Ok(())
    }

    fn prompted_at(&mut self, session: &str) -> io::Result<Option<i64>> {
        if self.fail_prompted {
            return Err(io::Error::other("injected"));
        }
        Ok(self.prompted.get(session).copied())
    }

    fn commit(
        &mut self,
        event: &str,
        session: &str,
        start: i64,
        now: i64,
    ) -> Result<(), CommitError> {
        self.calls += 1;
        if let Some(e) = self.force_commit.take() {
            return Err(e);
        }
        if self.events.contains(event) {
            return Err(CommitError::Duplicate);
        }
        let v = self.sessions.entry(session.to_string()).or_default();
        if v.iter().any(|t| *t > start) {
            return Err(CommitError::Duplicate);
        }
        if v.iter().filter(|t| now - **t < 86_400).count() as u32 >= MAX_RESUMES_PER_SESSION {
            return Err(CommitError::Cap);
        }
        self.events.insert(event.to_string());
        v.push(now);
        self.order.push("commit".into());
        Ok(())
    }
}

/// One scripted wake-up of `wait_event`. The clock moves `STEP` first.
#[derive(Clone)]
enum Act {
    /// The rotator writes this record: `Event::Swap`.
    Swap(SwapRecord),
    /// `swap.json` changes nothing (a rewrite, or an overflow): `Event::Swap`.
    Touch,
    /// The user prompts in `session` now: `Event::Prompted`.
    Prompt(&'static str),
    /// Auto-switch is turned off, then the rotator writes this record.
    OffThenSwap(SwapRecord),
    /// The user prompts in session `s`, then the rotator writes this record
    /// before the hook wakes: one `Event::Swap`.
    PromptThenSwap(SwapRecord),
    SessionGone,
    WatchLost,
}

struct Fake {
    on: bool,
    clock: i64,
    /// What the ledger shows (for the event key's live login).
    cycle: Option<CycleStatus>,
    /// The current `swap.json`, as `read_swap` returns it.
    swap: Option<SwapRecord>,
    /// Replaces `swap` right after the first `read_swap` (the snapshot):
    /// the rotator writing between the snapshot and the arm check.
    swap_race: Option<SwapRecord>,
    swap_reads: u32,
    script: VecDeque<Act>,
    guard: MemGuard,
    reads: Vec<i64>,
    logs: Vec<LogEntry>,
    panic_on_now: bool,
    rotator_down: bool,
    arm_err: bool,
    armed: bool,
    arms: u32,
    /// Deadlines passed to `wait_event`.
    waits: Vec<i64>,
    /// `$TMUX_PANE` of a verified pane, or none.
    pane_id: Option<&'static str>,
    pane_calls: u32,
    /// Keys sent successfully, in order.
    keys: Vec<Key>,
    key_attempts: usize,
    /// The send of this attempt (1-based) fails.
    fail_key: Option<usize>,
    /// The session exits once this many keys were sent.
    gone_after_keys: Option<usize>,
    /// What `stop_time` returns: the stop the transcript records (#2236).
    stop: Option<i64>,
    /// Each `stop_time` call: the transcript path it was given and `start`.
    stop_calls: Vec<(Option<PathBuf>, i64)>,
}

impl Fake {
    fn new() -> Self {
        Self {
            on: true,
            clock: START,
            cycle: Some(ledger("work")),
            swap: None,
            swap_race: None,
            swap_reads: 0,
            script: VecDeque::new(),
            guard: MemGuard::default(),
            reads: Vec::new(),
            logs: Vec::new(),
            panic_on_now: false,
            rotator_down: false,
            arm_err: false,
            armed: false,
            arms: 0,
            waits: Vec::new(),
            pane_id: None,
            pane_calls: 0,
            keys: Vec::new(),
            key_attempts: 0,
            fail_key: None,
            gone_after_keys: None,
            stop: None,
            stop_calls: Vec::new(),
        }
    }

    fn off() -> Self {
        Self {
            on: false,
            ..Self::new()
        }
    }

    fn with_pane() -> Self {
        Self {
            pane_id: Some("%5"),
            ..Self::new()
        }
    }

    fn then(mut self, act: Act) -> Self {
        self.script.push_back(act);
        self
    }

    fn outcomes(&self) -> Vec<Outcome> {
        self.logs.iter().map(|l| l.outcome).collect()
    }

    fn last_log(&self) -> &LogEntry {
        self.logs.last().expect("a log entry")
    }

    /// The hook starts again later, as a new `StopFailure` does.
    fn restart_at(&mut self, t: i64) {
        self.clock = t;
        self.armed = false;
    }
}

impl Deps for Fake {
    fn auto_switch_on(&mut self) -> bool {
        self.on
    }
    fn now(&mut self) -> i64 {
        assert!(!self.panic_on_now, "injected panic");
        self.clock
    }
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus> {
        self.reads.push(now);
        self.cycle.clone()
    }
    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }
    fn log(&mut self, _now: i64, entry: &LogEntry) {
        self.logs.push(entry.clone());
    }
    fn rotator_down(&mut self, _now: i64) -> bool {
        self.rotator_down
    }
    fn arm(&mut self) -> io::Result<()> {
        self.arms += 1;
        self.guard.order.push("arm".into());
        if self.arm_err {
            return Err(io::Error::other("injected"));
        }
        self.armed = true;
        Ok(())
    }
    fn wait_event(&mut self, deadline: i64) -> Event {
        assert!(self.armed, "wait_event before a successful arm");
        self.waits.push(deadline);
        self.guard.order.push("wait".into());
        assert!(
            self.waits.len() < 100,
            "the hook keeps waiting on a script that has ended"
        );
        let Some(act) = self.script.pop_front() else {
            self.clock = deadline;
            return Event::Deadline;
        };
        self.clock += STEP;
        match act {
            Act::Swap(r) => {
                self.swap = Some(r);
                Event::Swap
            }
            Act::Touch => Event::Swap,
            Act::Prompt(session) => {
                self.guard.prompted.insert(session_key(session), self.clock);
                Event::Prompted
            }
            Act::OffThenSwap(r) => {
                self.on = false;
                self.swap = Some(r);
                Event::Swap
            }
            Act::PromptThenSwap(r) => {
                self.guard.prompted.insert(session_key("s"), self.clock);
                self.swap = Some(r);
                Event::Swap
            }
            Act::SessionGone => Event::SessionGone,
            Act::WatchLost => Event::WatchLost,
        }
    }
    fn read_swap(&mut self, _now: i64) -> Option<SwapRecord> {
        self.swap_reads += 1;
        let r = self.swap.clone();
        if self.swap_reads == 1
            && let Some(next) = self.swap_race.take()
        {
            self.swap = Some(next);
        }
        r
    }
    fn stop_time(&mut self, transcript: Option<&Path>, start: i64) -> Option<i64> {
        // Reads nothing: the reader itself is tested in amplihack-utils.
        self.stop_calls
            .push((transcript.map(Path::to_path_buf), start));
        self.stop
    }
    fn pane(&mut self) -> Option<Pane> {
        self.pane_calls += 1;
        self.pane_id
            .and_then(|id| Pane::parse(Some(TMUX), Some(id)))
    }
    fn send_keys(&mut self, _pane: &Pane, key: Key) -> io::Result<()> {
        self.key_attempts += 1;
        self.guard.order.push(format!("key:{key:?}"));
        if self.fail_key == Some(self.key_attempts) {
            return Err(io::Error::other("injected"));
        }
        self.keys.push(key);
        Ok(())
    }
    fn session_alive_now(&mut self) -> bool {
        self.gone_after_keys.is_none_or(|n| self.keys.len() < n)
    }
}

fn ledger(live: &str) -> CycleStatus {
    CycleStatus {
        at: Some(START - 120),
        measured_at: Some(START - 150),
        outcome: Some("stayed".into()),
        alert: false,
        live: Some(live.into()),
        overflow: None,
        platform_live: false,
    }
}

/// A swap record `after` seconds past `START`, measured 30 s earlier.
fn record_at(seq: u64, after: i64, session: u8, week: u8) -> SwapRecord {
    SwapRecord {
        seq,
        live_login: "spare".into(),
        from_login: "work".into(),
        swapped_at: START + after,
        reading: SwapReading {
            session,
            week,
            measured_at: START + after - 30,
        },
    }
}

/// A swap after the stop, onto a login with room.
fn fresh(seq: u64) -> SwapRecord {
    record_at(seq, 600, 12, 40)
}

/// A swap from before the stop.
fn old(seq: u64) -> SwapRecord {
    record_at(seq, -600, 12, 40)
}

fn limit_input(session: &str, msg: &str) -> HookInput {
    serde_json::from_value(json!({
        "hook_event_name": "StopFailure",
        "session_id": session,
        "error": "rate_limit",
        "last_assistant_message": msg,
    }))
    .unwrap()
}

fn input(v: serde_json::Value) -> HookInput {
    serde_json::from_value(v).unwrap()
}

fn limit_payload() -> String {
    json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "last_assistant_message": "You've hit your session limit"
    })
    .to_string()
}

/// Nothing was resumed: no commit, no key.
fn assert_no_resume(d: &Fake) {
    assert!(d.guard.events.is_empty(), "no marker without a resume");
    assert!(d.guard.sessions.is_empty(), "no session resume recorded");
    assert_eq!(d.key_attempts, 0, "no key sent");
}

// ── Message and outcome strings ─────────────────────────────────────────────

#[test]
fn resume_message_is_the_documented_text() {
    assert_eq!(
        RESUME_MSG,
        "amplihack: the login rotator reports a login with capacity. \
         Continue the task you were working on; if the usage limit is still in place, stop and wait."
    );
    assert!(!RESUME_MSG.contains('\n'));
}

#[test]
fn outcome_strings_and_exit_codes() {
    let table = [
        (Outcome::Resumed, "resumed", 2),
        (Outcome::ResumedKeys, "resumed-keys", 0),
        (Outcome::ResumeKeysFailed, "resume-keys-failed", 0),
        (Outcome::WatchLost, "watch-lost", 0),
        (Outcome::SwapStale, "swap-stale", 0),
        (Outcome::SwapNoRoom, "swap-no-room", 0),
        (Outcome::NoCapacityTimeout, "no-capacity-timeout", 0),
        (Outcome::MovedOn, "moved-on", 0),
        (Outcome::Duplicate, "duplicate", 0),
        (Outcome::Cap, "cap", 0),
        (Outcome::StopHookActive, "stop-hook-active", 0),
        (Outcome::GuardFailed, "guard-failed", 0),
        (Outcome::SessionGone, "session-gone", 0),
        (Outcome::RotatorDown, "rotator-down", 0),
        (Outcome::Disabled, "disabled", 0),
        (Outcome::NotLimit, "not-limit", 0),
    ];
    for (o, s, code) in table {
        assert_eq!(o.as_str(), s);
        assert_eq!(o.exit_code(), code, "{s}");
    }
}

#[test]
fn per_session_cap_is_three() {
    assert_eq!(MAX_RESUMES_PER_SESSION, 3);
}

// ── is_limit (IV-3) ─────────────────────────────────────────────────────────

#[test]
fn is_limit_is_exactly_rate_limit() {
    assert!(is_limit(Some("rate_limit")));
    for e in [
        None,
        Some(""),
        Some("unknown"),
        Some("overloaded"),
        Some("authentication_failed"),
        Some("server_error"),
        Some("RATE_LIMIT"),
        Some("rate_limit "),
    ] {
        assert!(!is_limit(e), "{e:?}");
    }
}

#[test]
fn limit_text_without_rate_limit_error_is_not_a_limit() {
    for error in [json!(null), json!(""), json!("unknown")] {
        let mut d = Fake::new().then(Act::Swap(fresh(1)));
        let out = run_with(
            &input(json!({
                "hook_event_name": "StopFailure",
                "session_id": "s",
                "error": error,
                "error_details": "Stop and wait for limit to reset",
                "last_assistant_message": "You've hit your weekly limit"
            })),
            &mut d,
        );
        assert_eq!(out, Outcome::NotLimit, "{error}");
        assert_untouched(&d);
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[test]
fn event_key_is_stable_lowercase_sha256_hex() {
    let k = event_key("s", Some("rate_limit"), Some("d"), Some("m"), Some("p"));
    assert!(is_hex64(&k), "{k}");
    assert_eq!(
        k,
        event_key("s", Some("rate_limit"), Some("d"), Some("m"), Some("p"))
    );
}

#[test]
fn event_key_changes_with_every_field() {
    let base = event_key("s", Some("rate_limit"), Some("d"), Some("m"), Some("p"));
    for other in [
        event_key("t", Some("rate_limit"), Some("d"), Some("m"), Some("p")),
        event_key("s", Some("rate_limited"), Some("d"), Some("m"), Some("p")),
        event_key("s", Some("rate_limit"), Some("e"), Some("m"), Some("p")),
        event_key("s", Some("rate_limit"), Some("d"), Some("n"), Some("p")),
        event_key("s", Some("rate_limit"), Some("d"), Some("m"), Some("q")),
        event_key("s", Some("rate_limit"), Some("d"), Some("m"), None),
    ] {
        assert_ne!(base, other);
    }
}

#[test]
fn event_key_fields_are_separated_unambiguously() {
    assert_ne!(
        event_key("ab", Some("c"), None, None, None),
        event_key("a", Some("bc"), None, None, None)
    );
    assert_ne!(
        event_key("s", None, Some("xy"), Some("z"), None),
        event_key("s", None, Some("x"), Some("yz"), None)
    );
    assert_ne!(
        event_key("s", None, None, Some("mp"), None),
        event_key("s", None, None, Some("m"), Some("p"))
    );
}

#[test]
fn session_key_is_hex_and_hides_the_session_id() {
    let id = "session-ABC-123";
    let k = session_key(id);
    assert!(is_hex64(&k));
    assert!(!k.contains(id));
    assert_ne!(k, event_key(id, None, None, None, None));
    assert_ne!(k, session_key("session-ABC-124"));
}

#[test]
fn session_id_rules() {
    assert!(session_id_ok(Some("abc")));
    assert!(session_id_ok(Some("a".repeat(256).as_str())));
    assert!(!session_id_ok(Some("a".repeat(257).as_str())));
    assert!(!session_id_ok(Some("")));
    assert!(!session_id_ok(None));
}

// ── gate: contract order ────────────────────────────────────────────────────

#[test]
fn gate_order_is_disabled_notlimit_stopactive_duplicate_cap_proceed() {
    let all_bad = Facts {
        enabled: false,
        limit: false,
        session_ok: false,
        stop_hook_active: true,
        duplicate: true,
        capped: true,
    };
    assert_eq!(gate(&all_bad), Gate::Disabled);
    let f = Facts {
        enabled: true,
        ..all_bad
    };
    assert_eq!(gate(&f), Gate::NotLimit);
    let f = Facts { limit: true, ..f };
    assert_eq!(gate(&f), Gate::NotLimit, "a bad session id is NotLimit");
    let f = Facts {
        session_ok: true,
        ..f
    };
    assert_eq!(gate(&f), Gate::StopHookActive);
    let f = Facts {
        stop_hook_active: false,
        ..f
    };
    assert_eq!(gate(&f), Gate::Duplicate);
    let f = Facts {
        duplicate: false,
        ..f
    };
    assert_eq!(gate(&f), Gate::Cap);
    let f = Facts { capped: false, ..f };
    assert_eq!(gate(&f), Gate::Proceed);
}

// ── (a) A swap record after the stop resumes once ───────────────────────────

#[test]
fn a_without_a_pane_a_swap_record_resumes_with_exit_2() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s1", "You've hit your weekly limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert_eq!(out.exit_code(), 2);
    assert_eq!(d.guard.events.len(), 1, "event marker committed");
    assert_eq!(d.guard.sessions.values().map(Vec::len).sum::<usize>(), 1);
    assert_eq!(d.key_attempts, 0, "exit 2 only: no keys");
    assert_eq!(d.outcomes(), [Outcome::Resumed]);
    assert_eq!(
        d.reads.len(),
        1,
        "the ledger is read once, for the event key"
    );
}

#[test]
fn a_with_a_pane_a_swap_record_resumes_with_the_four_keys_and_exit_0() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s1", "You've hit your weekly limit"), &mut d);
    assert_eq!(out, Outcome::ResumedKeys);
    assert_eq!(out.exit_code(), 0);
    assert_eq!(d.keys, RESUME_KEYS);
    assert_eq!(d.key_attempts, 4);
    assert_eq!(d.guard.events.len(), 1, "committed");
}

#[test]
fn a_the_resume_is_committed_before_the_first_key() {
    // S6: commit runs under the lock before anything reaches the pane.
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    run_with(&limit_input("s", "hit your limit"), &mut d);
    let commit = d.guard.order.iter().position(|o| o == "commit").unwrap();
    let first_key = d
        .guard
        .order
        .iter()
        .position(|o| o.starts_with("key:"))
        .unwrap();
    assert!(commit < first_key, "{:?}", d.guard.order);
    let arm = d.guard.order.iter().position(|o| o == "arm").unwrap();
    let wait = d.guard.order.iter().position(|o| o == "wait").unwrap();
    assert!(arm < wait, "{:?}", d.guard.order);
}

#[test]
fn a_the_wait_is_for_the_lifetime_cap() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(d.waits, [DEADLINE], "one wait, until START + MAX_WAIT");
}

#[test]
fn a_resumed_keys_log_entry_names_the_swap_pane_and_keys() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(7)));
    run_with(&limit_input("secret-session-id", "msg"), &mut d);
    let e = d.last_log();
    assert_eq!(e.outcome, Outcome::ResumedKeys);
    assert_eq!(e.session, session_key("secret-session-id")[..12]);
    assert_eq!(
        e.event,
        event_key(
            "secret-session-id",
            Some("rate_limit"),
            None,
            Some("msg"),
            Some("work")
        )[..12]
    );
    assert_eq!(e.swap_seq, Some(7));
    assert_eq!(e.swap_live.as_deref(), Some("spare"));
    assert_eq!(e.keys_sent, Some(4));
    let tag = Pane::parse(Some(TMUX), Some("%5")).unwrap().log_tag();
    assert_eq!(e.pane.as_deref(), Some(tag.as_str()));
    assert!(
        !e.pane.as_deref().unwrap().contains('%'),
        "the tag, not the id"
    );
}

#[test]
fn a_resumed_exit_2_log_entry_has_no_pane_or_keys() {
    let mut d = Fake::new().then(Act::Swap(fresh(3)));
    run_with(&limit_input("s", "hit your limit"), &mut d);
    let e = d.last_log();
    assert_eq!(e.outcome, Outcome::Resumed);
    assert_eq!(e.swap_seq, Some(3));
    assert_eq!(e.pane, None);
    assert_eq!(e.keys_sent, None);
}

#[test]
fn a_record_written_between_the_snapshot_and_the_wait_resumes_without_an_event() {
    // D10: the hook reads swap.json once after arming, so a swap that
    // landed before the watch existed is not missed.
    let mut d = Fake::new();
    d.swap = Some(old(4));
    d.swap_race = Some(fresh(5));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert!(d.waits.is_empty(), "no wait: {:?}", d.waits);
    assert_eq!(d.arms, 1, "armed before the check");
}

#[test]
fn a_swap_between_the_stop_and_the_hook_start_resumes_at_once() {
    // Crusty round 1 on #2141: the rotator finishes a swap after the
    // session stopped but before the hook took its snapshot, in the same
    // second. The snapshot is that swap, and the session stopped on the
    // login swapped away from, so the hook resumes without waiting.
    let mut d = Fake::new();
    d.swap = Some(record_at(6, 0, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert!(d.waits.is_empty(), "no wait: {:?}", d.waits);
}

#[test]
fn a_snapshot_from_before_the_stop_waits_for_the_next_swap() {
    let mut d = Fake::new();
    d.swap = Some(record_at(6, -2, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(d.waits, [DEADLINE]);
}

#[test]
fn a_a_reset_seq_with_fresh_times_resumes() {
    // D4: seq is not trusted to only go up.
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    d.swap = Some(old(41));
    assert_eq!(
        run_with(&limit_input("s", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

// ── (b) Records that never resume ───────────────────────────────────────────

#[test]
fn b_no_record_before_the_deadline_is_no_capacity_timeout() {
    let mut d = Fake::with_pane();
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(out.exit_code(), 0);
    assert_no_resume(&d);
    assert_eq!(d.waits, [DEADLINE]);
    assert_eq!(d.outcomes(), [Outcome::NoCapacityTimeout]);
}

#[test]
fn b_a_record_from_before_the_stop_never_resumes() {
    // D5: a stop on the new login waits for the next swap.
    let mut d = Fake::with_pane();
    d.swap = Some(old(9));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_no_resume(&d);
}

#[test]
fn b_an_unchanged_record_on_an_event_keeps_waiting() {
    let mut d = Fake::with_pane()
        .then(Act::Touch)
        .then(Act::Touch)
        .then(Act::Swap(fresh(10)));
    d.swap = Some(old(9));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::ResumedKeys);
    assert_eq!(d.waits.len(), 3, "woke three times, resumed on the third");
}

#[test]
fn b_a_new_record_swapped_before_the_stop_is_stale_and_logged() {
    let mut d = Fake::with_pane().then(Act::Swap(old(2)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout, "the wait went on");
    assert_no_resume(&d);
    assert_eq!(
        d.outcomes(),
        [Outcome::SwapStale, Outcome::NoCapacityTimeout]
    );
    assert_eq!(d.logs[0].swap_seq, Some(2));
}

#[test]
fn b_a_stop_between_measurement_and_swap_is_resumed() {
    // The rotator measured before the stop and swapped after it: the session
    // stopped on the login swapped away from, so the swap resumes it.
    let mut r = fresh(2);
    r.reading.measured_at = START - 30;
    let mut d = Fake::new().then(Act::Swap(r));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert!(!d.outcomes().contains(&Outcome::SwapStale));
}

#[test]
fn b_a_swap_onto_a_full_login_is_no_room_and_the_wait_goes_on() {
    let mut d = Fake::with_pane()
        .then(Act::Swap(record_at(2, 300, 100, 20)))
        .then(Act::Swap(record_at(3, 900, 20, 100)))
        .then(Act::Swap(record_at(4, 1500, 99, 99)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::ResumedKeys);
    assert_eq!(
        d.outcomes(),
        [
            Outcome::SwapNoRoom,
            Outcome::SwapNoRoom,
            Outcome::ResumedKeys
        ]
    );
    assert_eq!(d.last_log().swap_seq, Some(4));
}

#[test]
fn b_run_with_never_returns_a_log_only_outcome() {
    for act in [
        Act::Swap(old(2)),
        Act::Swap(record_at(2, 300, 100, 100)),
        Act::Touch,
    ] {
        let mut d = Fake::new().then(act);
        let out = run_with(&limit_input("s", "hit your limit"), &mut d);
        assert!(
            !matches!(out, Outcome::SwapStale | Outcome::SwapNoRoom),
            "{out:?}"
        );
    }
}

#[test]
fn b_a_ledger_with_capacity_but_no_swap_record_never_resumes() {
    // #2085: the trigger is the swap record alone; a stayed or switched
    // ledger after the stop is not one (D2).
    for outcome in ["stayed", "swapped", "switched"] {
        let mut d = Fake::new();
        d.cycle = Some(CycleStatus {
            at: Some(START + 120),
            measured_at: Some(START + 90),
            outcome: Some(outcome.into()),
            ..ledger("spare")
        });
        let out = run_with(&limit_input("s", "hit your limit"), &mut d);
        assert_eq!(out, Outcome::NoCapacityTimeout, "{outcome}");
        assert_no_resume(&d);
    }
}

// ── D7: exactly one resume path ─────────────────────────────────────────────

#[test]
fn run_with_io_resumed_keys_exits_0_with_nothing_on_stderr() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    let mut stderr = Vec::new();
    let code = run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d);
    assert_eq!(code, 0);
    assert!(stderr.is_empty(), "no exit-2 wake after keys");
    assert_eq!(d.keys, RESUME_KEYS);
}

#[test]
fn run_with_io_without_a_pane_exits_2_with_the_message_and_sends_nothing() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let mut stderr = Vec::new();
    let code = run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d);
    assert_eq!(code, 2);
    assert_eq!(stderr, format!("{RESUME_MSG}\n").as_bytes());
    assert_eq!(d.key_attempts, 0);
}

#[test]
fn a_pane_that_fails_verification_takes_the_exit_2_path() {
    // `pane()` returns only a verified pane; none means exit 2, no keys.
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    d.pane_id = None;
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert!(d.pane_calls >= 1, "the pane is checked before the commit");
    assert_eq!(d.key_attempts, 0);
}

#[test]
fn the_pane_is_checked_before_the_commit() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    d.guard.force_commit = Some(CommitError::Duplicate);
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Duplicate);
    assert_eq!(d.key_attempts, 0, "a lost commit sends nothing");
}

// ── D8: a failure part-way through the keys stops the rest ──────────────────

#[test]
fn a_key_that_fails_stops_the_rest_and_never_falls_back_to_exit_2() {
    for k in 1..=4 {
        let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
        d.fail_key = Some(k);
        let mut stderr = Vec::new();
        let code = run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d);
        assert_eq!(code, 0, "key {k}");
        assert!(stderr.is_empty(), "key {k}: no fallback to exit 2");
        assert_eq!(d.key_attempts, k, "key {k}: nothing after the failure");
        assert_eq!(d.keys, RESUME_KEYS[..k - 1], "key {k}");
        assert_eq!(d.guard.events.len(), 1, "key {k}: the commit stands");
        let e = d.last_log();
        assert_eq!(e.outcome, Outcome::ResumeKeysFailed, "key {k}");
        assert_eq!(e.keys_sent, Some((k - 1) as u8), "key {k}");
        assert!(e.detail.is_some(), "key {k}: the error kind is logged");
    }
}

#[test]
fn the_session_exiting_mid_keys_stops_the_rest() {
    // S4: liveness is checked before each key.
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    d.gone_after_keys = Some(2);
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::ResumeKeysFailed);
    assert_eq!(d.keys, RESUME_KEYS[..2]);
    assert_eq!(d.key_attempts, 2, "no third key once the session is gone");
    let e = d.last_log();
    assert_eq!(e.keys_sent, Some(2));
    assert_eq!(e.detail.as_deref(), Some("session-gone"));
}

#[test]
fn send_resume_sends_the_four_keys_in_order() {
    let mut d = Fake::new();
    let pane = Pane::parse(Some(TMUX), Some("%9")).unwrap();
    assert!(send_resume(&mut d, &pane).is_ok());
    assert_eq!(d.keys, RESUME_KEYS);
}

#[test]
fn send_resume_reports_how_many_keys_were_sent() {
    let mut d = Fake::new();
    d.fail_key = Some(3);
    let pane = Pane::parse(Some(TMUX), Some("%9")).unwrap();
    let err = send_resume(&mut d, &pane).unwrap_err();
    assert_eq!(err.sent, 2);
    assert_eq!(err.err.kind(), io::ErrorKind::Other);
    assert_eq!(d.key_attempts, 3);
}

#[test]
fn send_resume_sends_nothing_to_a_session_that_is_gone() {
    let mut d = Fake::new();
    d.gone_after_keys = Some(0);
    let pane = Pane::parse(Some(TMUX), Some("%9")).unwrap();
    let err = send_resume(&mut d, &pane).unwrap_err();
    assert_eq!(err.sent, 0);
    assert_eq!(d.key_attempts, 0);
}

// ── D10: watches ────────────────────────────────────────────────────────────

#[test]
fn a_watch_that_cannot_be_armed_is_watch_lost_and_never_resumes() {
    let mut d = Fake::with_pane();
    d.arm_err = true;
    // Even a record that would resume is not acted on without a watch.
    d.swap_race = Some(fresh(5));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::WatchLost);
    assert_eq!(out.exit_code(), 0);
    assert!(d.waits.is_empty(), "no wait without a watch");
    assert_no_resume(&d);
    assert_eq!(d.outcomes(), [Outcome::WatchLost]);
}

#[test]
fn a_watch_lost_mid_wait_ends_the_wait_without_a_resume() {
    let mut d = Fake::with_pane()
        .then(Act::Touch)
        .then(Act::WatchLost)
        .then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::WatchLost);
    assert_no_resume(&d);
    assert_eq!(d.waits.len(), 2, "nothing after the lost watch");
}

#[test]
fn the_wait_ends_when_its_session_exits() {
    let mut d = Fake::with_pane()
        .then(Act::SessionGone)
        .then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::SessionGone);
    assert_eq!(out.exit_code(), 0);
    assert_no_resume(&d);
    assert_eq!(d.outcomes(), [Outcome::SessionGone]);
}

// ── (c) repeated distinct events are capped per session ─────────────────────

#[test]
fn c_distinct_limit_events_stop_at_the_session_cap() {
    let mut d = Fake::new();
    let mut resumed = 0;
    for i in 0..6u64 {
        // Each stop is met by a new swap after it.
        let t0 = d.clock;
        d.restart_at(t0);
        d.swap_race = None;
        let mut r = fresh(i + 1);
        r.swapped_at = t0 + 600;
        r.reading.measured_at = t0 + 570;
        d.script.push_back(Act::Swap(r));
        let out = run_with(&limit_input("s1", &format!("hit your limit #{i}")), &mut d);
        if out == Outcome::Resumed {
            resumed += 1;
        } else {
            assert_eq!(out, Outcome::Cap, "event {i}");
            d.script.clear();
        }
    }
    assert_eq!(resumed, MAX_RESUMES_PER_SESSION);
}

#[test]
fn c_capped_session_exits_without_waiting() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    d.guard
        .sessions
        .insert(session_key("s1"), vec![START - 10, START - 20, START - 30]);
    let out = run_with(&limit_input("s1", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Cap);
    assert_eq!(d.arms, 0, "no watch is armed");
    assert!(d.waits.is_empty());
    assert_eq!(d.key_attempts, 0);
    assert_eq!(d.outcomes(), [Outcome::Cap]);
}

#[test]
fn c_other_sessions_are_not_capped_by_this_one() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    d.guard
        .sessions
        .insert(session_key("s1"), vec![START - 10, START - 20, START - 30]);
    assert_eq!(
        run_with(&limit_input("s2", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

// ── (d) non-limit → no action ───────────────────────────────────────────────

fn assert_untouched(d: &Fake) {
    assert!(d.reads.is_empty(), "ledger read");
    assert_eq!(d.swap_reads, 0, "swap record read");
    assert_eq!(d.arms, 0, "armed");
    assert!(d.waits.is_empty(), "waited");
    assert_eq!(d.guard.calls, 0, "guard touched");
    assert!(d.logs.is_empty(), "logged");
    assert_eq!(d.key_attempts, 0, "keys sent");
}

#[test]
fn d_non_limit_error_is_ignored_even_with_matching_text() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    let out = run_with(
        &input(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "overloaded",
            "last_assistant_message": "You've hit your weekly limit"
        })),
        &mut d,
    );
    assert_eq!(out, Outcome::NotLimit);
    assert_untouched(&d);
}

#[test]
fn d_prime_no_error_is_ignored() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let out = run_with(
        &input(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "last_assistant_message": "Here is the summary."
        })),
        &mut d,
    );
    assert_eq!(out, Outcome::NotLimit);
    assert_untouched(&d);
}

#[test]
fn d_missing_or_oversized_session_id_is_ignored() {
    for session in [json!(null), json!(""), json!("x".repeat(257))] {
        let mut d = Fake::new().then(Act::Swap(fresh(1)));
        let out = run_with(
            &input(json!({
                "hook_event_name": "StopFailure",
                "session_id": session,
                "error": "rate_limit"
            })),
            &mut d,
        );
        assert_eq!(out, Outcome::NotLimit);
        assert_untouched(&d);
    }
}

#[test]
fn d_other_hook_events_are_ignored() {
    for v in [
        json!({"hook_event_name": "Stop", "session_id": "s", "error": "rate_limit"}),
        json!({"hook_event_name": "Notification", "session_id": "s"}),
    ] {
        let mut d = Fake::new().then(Act::Swap(fresh(1)));
        assert_eq!(run_with(&input(v), &mut d), Outcome::NotLimit);
        assert_untouched(&d);
    }
}

// ── (e) auto-switch off ─────────────────────────────────────────────────────

#[test]
fn e_auto_switch_off_does_nothing() {
    let mut d = Fake::off().then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Disabled);
    assert_eq!(out.exit_code(), 0);
    assert_untouched(&d);
}

#[test]
fn e_auto_switch_off_prints_nothing() {
    let mut d = Fake::off().then(Act::Swap(fresh(1)));
    let mut stderr = Vec::new();
    let code = run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d);
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert_untouched(&d);
}

#[test]
fn auto_switch_turned_off_while_waiting_stops_without_keys() {
    let mut d = Fake::with_pane().then(Act::OffThenSwap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Disabled);
    assert_eq!(out.exit_code(), 0);
    assert_no_resume(&d);
    assert_eq!(d.outcomes(), [Outcome::Disabled]);
}

// ── stop_hook_active, duplicate, guard failures ─────────────────────────────

#[test]
fn stop_hook_active_never_resumes_and_touches_no_guard_state() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    let out = run_with(
        &input(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "rate_limit",
            "stop_hook_active": true
        })),
        &mut d,
    );
    assert_eq!(out, Outcome::StopHookActive);
    assert_eq!(d.guard.calls, 0, "checked before any guard I/O");
    assert!(d.stop_calls.is_empty(), "the transcript is not read");
    assert_eq!(d.arms, 0);
    assert_eq!(d.key_attempts, 0);
    assert_eq!(d.outcomes(), [Outcome::StopHookActive]);
}

#[test]
fn duplicate_event_exits_without_waiting() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let input = limit_input("s", "hit your limit");
    assert_eq!(run_with(&input, &mut d), Outcome::Resumed);
    let (arms, waits) = (d.arms, d.waits.len());
    d.restart_at(d.clock);
    d.script.push_back(Act::Swap(fresh(2)));
    assert_eq!(run_with(&input, &mut d), Outcome::Duplicate);
    assert_eq!(d.arms, arms, "a duplicate arms nothing");
    assert_eq!(d.waits.len(), waits, "a duplicate does not wait");
}

#[test]
fn guard_failure_before_waiting_is_guard_failed() {
    for which in ["seen", "resumes", "prepare"] {
        let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
        match which {
            "seen" => d.guard.fail_seen = true,
            "resumes" => d.guard.fail_resumes = true,
            _ => d.guard.fail_prepare = true,
        }
        let out = run_with(&limit_input("s", "hit your limit"), &mut d);
        assert_eq!(out, Outcome::GuardFailed, "{which}");
        assert_eq!(out.exit_code(), 0);
        assert_eq!(d.arms, 0, "{which}: no watch after a guard failure");
        assert!(d.waits.is_empty(), "{which}");
        assert_eq!(d.key_attempts, 0, "{which}");
    }
}

#[test]
fn losing_the_commit_race_never_resumes() {
    for (err, want) in [
        (CommitError::Duplicate, Outcome::Duplicate),
        (CommitError::Cap, Outcome::Cap),
        (CommitError::Io, Outcome::GuardFailed),
    ] {
        let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
        d.guard.force_commit = Some(err);
        let mut stderr = Vec::new();
        let code = run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d);
        assert_eq!(code, 0, "{want:?}");
        assert!(stderr.is_empty(), "{want:?}");
        assert_eq!(d.key_attempts, 0, "{want:?}");
        assert_eq!(d.last_log().outcome, want);
    }
}

// ── No infinite continue loop ───────────────────────────────────────────────

#[test]
fn loop_one_swap_record_resumes_at_most_once() {
    // The woken session hits a limit again before any new swap: the record
    // it was resumed on is its snapshot now, so it waits for the next swap.
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    assert_eq!(
        run_with(&limit_input("s", "first limit"), &mut d),
        Outcome::Resumed
    );
    for i in 0..10 {
        d.restart_at(d.clock + 1);
        d.script.push_back(Act::Touch);
        let out = run_with(&limit_input("s", &format!("limit again {i}")), &mut d);
        assert_eq!(out, Outcome::NoCapacityTimeout, "invocation {i}");
    }
    assert_eq!(d.guard.events.len(), 1);
}

#[test]
fn loop_a_limit_again_within_a_second_of_the_swap_does_not_resume_twice() {
    // The swap is written in the second the hook resumes the session, and
    // the session hits a limit again in the next second. That swap is within
    // the stop allowance, but the session was resumed on it, so it waits.
    let mut d = Fake::new().then(Act::Swap(record_at(1, STEP, 12, 40)));
    assert_eq!(
        run_with(&limit_input("s", "first limit"), &mut d),
        Outcome::Resumed
    );
    assert_eq!(d.clock, START + STEP, "resumed in the swap's second");
    d.restart_at(d.clock + 1);
    let out = run_with(&limit_input("s", "limit again"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(d.guard.events.len(), 1);
}

#[test]
fn loop_fifty_invocations_on_one_event_resume_at_most_once() {
    let mut d = Fake::new();
    let mut resumed = 0;
    for i in 0..50u64 {
        d.restart_at(d.clock + 1);
        let mut r = fresh(i + 1);
        r.swapped_at = d.clock + 600;
        r.reading.measured_at = d.clock + 570;
        d.script.clear();
        d.script.push_back(Act::Swap(r));
        if run_with(&limit_input("s", "You've hit your weekly limit"), &mut d) == Outcome::Resumed {
            resumed += 1;
        }
    }
    assert_eq!(resumed, 1);
}

#[test]
fn loop_stop_hook_active_invocations_never_resume() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    for i in 0..50 {
        let out = run_with(
            &input(json!({
                "hook_event_name": "StopFailure",
                "session_id": "s",
                "error": "rate_limit",
                "last_assistant_message": format!("hit your limit {i}"),
                "stop_hook_active": true
            })),
            &mut d,
        );
        assert_ne!(out, Outcome::Resumed);
        assert_ne!(out, Outcome::ResumedKeys);
    }
    assert!(d.guard.events.is_empty());
    assert_eq!(d.key_attempts, 0);
}

// ── Rotator down ────────────────────────────────────────────────────────────

#[test]
fn rotator_down_at_the_start_ends_without_waiting() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    d.rotator_down = true;
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::RotatorDown);
    assert_eq!(out.exit_code(), 0);
    assert_eq!(d.arms, 0);
    assert_no_resume(&d);
    assert_eq!(d.outcomes(), [Outcome::RotatorDown]);
}

#[test]
fn a_duplicate_event_is_reported_before_a_down_rotator() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let input = limit_input("s", "hit your limit");
    assert_eq!(run_with(&input, &mut d), Outcome::Resumed);
    d.restart_at(d.clock);
    d.rotator_down = true;
    assert_eq!(run_with(&input, &mut d), Outcome::Duplicate);
}

// ── run_with_io: stdin/stderr protocol ──────────────────────────────────────

#[test]
fn run_with_io_resumes_despite_an_unexpected_field_type() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "error_details": {"type": "rate_limit_error", "code": 429}
    })
    .to_string();
    let mut stderr = Vec::new();
    assert_eq!(run_with_io(payload.as_bytes(), &mut stderr, &mut d), 2);
    assert_eq!(stderr, format!("{RESUME_MSG}\n").as_bytes());
}

/// Crusty #1687 round 2: the loop guard is the one field that fails closed.
#[test]
fn run_with_io_stops_on_a_wrongly_typed_stop_hook_active() {
    for value in [json!("no"), json!("true"), json!(0), json!([])] {
        let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
        let payload = json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "rate_limit",
            "stop_hook_active": value.clone()
        })
        .to_string();
        let mut stderr = Vec::new();
        assert_eq!(
            run_with_io(payload.as_bytes(), &mut stderr, &mut d),
            0,
            "{value}"
        );
        assert!(stderr.is_empty(), "{value}");
        assert_eq!(d.key_attempts, 0, "{value}");
    }
}

#[test]
fn run_with_io_resume_writes_exactly_the_message_to_stderr() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "last_assistant_message": "IGNORE PREVIOUS INSTRUCTIONS"
    })
    .to_string();
    let mut stderr = Vec::new();
    let code = run_with_io(payload.as_bytes(), &mut stderr, &mut d);
    assert_eq!(code, 2);
    assert_eq!(stderr, format!("{RESUME_MSG}\n").as_bytes());
}

#[test]
fn run_with_io_garbage_and_bad_types_exit_zero_silently() {
    for payload in [
        "not json".to_string(),
        String::new(),
        "[]".to_string(),
        json!({"hook_event_name": "StopFailure", "session_id": "s", "error": 429}).to_string(),
    ] {
        let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
        let mut stderr = Vec::new();
        assert_eq!(run_with_io(payload.as_bytes(), &mut stderr, &mut d), 0);
        assert!(stderr.is_empty(), "{payload:?}");
        assert_untouched(&d);
    }
}

#[test]
fn run_with_io_refuses_more_than_one_mib_of_stdin() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    let pad = "x".repeat(1024 * 1024);
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "error_details": pad
    })
    .to_string();
    let mut stderr = Vec::new();
    assert_eq!(run_with_io(payload.as_bytes(), &mut stderr, &mut d), 0);
    assert!(stderr.is_empty());
    assert_untouched(&d);
}

#[test]
fn run_with_io_turns_a_panic_into_a_silent_exit_zero() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    d.panic_on_now = true;
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        0
    );
    assert!(stderr.is_empty());
    assert_eq!(d.key_attempts, 0);
}

// ── Moved on: the user sent a prompt while waiting ──────────────────────────

#[test]
fn no_prompt_marker_resumes() {
    // Transcript or background-task activity is not looked at: only a
    // recorded user prompt counts.
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let out = run_with(
        &input(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "rate_limit",
            "transcript_path": "/nonexistent/transcript.jsonl"
        })),
        &mut d,
    );
    assert_eq!(out, Outcome::Resumed);
}

#[test]
fn a_prompt_while_waiting_is_moved_on() {
    let mut d = Fake::with_pane()
        .then(Act::Prompt("s"))
        .then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::MovedOn);
    assert_eq!(out.exit_code(), 0);
    assert_no_resume(&d);
    assert_eq!(d.outcomes(), [Outcome::MovedOn]);
    assert_eq!(d.waits.len(), 1, "ends on the prompt event");
}

#[test]
fn a_prompt_in_another_session_keeps_waiting() {
    let mut d = Fake::new()
        .then(Act::Prompt("other"))
        .then(Act::Swap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert_eq!(d.waits.len(), 2);
}

#[test]
fn a_prompt_just_before_the_swap_is_moved_on() {
    // Moved-on is checked again after a swap event, before the commit.
    let mut d = Fake::with_pane().then(Act::PromptThenSwap(fresh(1)));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::MovedOn);
    assert_no_resume(&d);
}

#[test]
fn prompt_before_the_start_still_resumes() {
    // The prompt that started the limited turn is older than the hook.
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    d.guard.prompted.insert(session_key("s"), START - 30);
    assert_eq!(
        run_with(&limit_input("s", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

#[test]
fn prompt_in_the_start_second_is_not_moved_on() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    d.guard.prompted.insert(session_key("s"), START);
    assert_eq!(
        run_with(&limit_input("s", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

#[test]
fn resume_state_is_prepared_once_before_arming() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    assert_eq!(
        run_with(&limit_input("s", "hit your limit"), &mut d),
        Outcome::Resumed
    );
    assert_eq!(d.guard.prepared, 1);
    assert_eq!(d.arms, 1);
}

#[test]
fn unreadable_prompt_marker_is_guard_failed() {
    let mut d = Fake::with_pane().then(Act::Swap(fresh(1)));
    d.guard.fail_prompted = true;
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::GuardFailed);
    assert_no_resume(&d);
}

// ── Event key: the live login at the limit ──────────────────────────────────

#[test]
fn same_login_limited_again_is_the_same_event() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let msg = "You've hit your session limit";
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);
    d.restart_at(d.clock);
    d.script.push_back(Act::Swap(fresh(2)));
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Duplicate);
}

#[test]
fn limit_after_a_swap_is_a_new_event() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let msg = "You've hit your session limit";
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);
    // The ledger now names `spare`: the same text again is a new limit,
    // resumed by the next swap.
    d.cycle = Some(ledger("spare"));
    let t0 = d.clock + 1;
    d.restart_at(t0);
    let mut r = fresh(2);
    r.swapped_at = t0 + 600;
    r.reading.measured_at = t0 + 570;
    d.script.push_back(Act::Swap(r));
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);
    assert_eq!(d.guard.events.len(), 2);
}

// ── One wake per session ────────────────────────────────────────────────────

#[test]
fn two_waiters_for_one_session_with_distinct_events_resume_once() {
    // Waiter B (started at START + 10) is woken first and commits; waiter A
    // (started at START) sees the next swap and loses: the session already
    // resumed after A started.
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    d.clock = START + 10;
    let b = run_with(&limit_input("s", "first limit text"), &mut d);
    d.restart_at(START);
    d.swap = None;
    d.script.push_back(Act::Swap(fresh(2)));
    let a = run_with(&limit_input("s", "second limit text"), &mut d);
    let outs = [a, b];
    assert_eq!(outs.iter().filter(|o| **o == Outcome::Resumed).count(), 1);
    assert!(outs.contains(&Outcome::Duplicate), "{outs:?}");
    assert_eq!(d.guard.sessions[&session_key("s")].len(), 1);
}

// ── Issue #2236: the stop time from the transcript ──────────────────────────

/// The stop the transcript records, ten seconds before the hook started.
const STOP: i64 = START - 10;

#[test]
fn the_transcript_path_from_the_payload_is_passed_to_stop_time_once() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let out = run_with(
        &input(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "rate_limit",
            "last_assistant_message": "hit your limit",
            "transcript_path": "/nonexistent/t.jsonl",
        })),
        &mut d,
    );
    assert_eq!(out, Outcome::Resumed);
    assert_eq!(
        d.stop_calls,
        [(Some(PathBuf::from("/nonexistent/t.jsonl")), START)],
        "read once, with the hook's start"
    );
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(d.stop_calls, [(None, START)], "no path in the payload");
}

#[test]
fn issue_2236_a_snapshot_after_a_known_stop_resumes_without_waiting() {
    // The hook started ten seconds after the stop and noted a swap made
    // three seconds after it, onto a login with room.
    let mut d = Fake::new();
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, -7, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert!(d.waits.is_empty(), "no wait: {:?}", d.waits);
    assert_eq!(d.guard.events.len(), 1);
}

#[test]
fn issue_2236_without_a_stop_the_same_snapshot_still_waits() {
    // The fallback is today's rule: seven seconds before the start is
    // before the stop.
    let mut d = Fake::new();
    d.swap = Some(record_at(6, -7, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(d.waits, [DEADLINE]);
    assert_no_resume(&d);
}

#[test]
fn issue_2236_a_snapshot_before_a_known_stop_waits() {
    let mut d = Fake::new();
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, -15, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert!(!d.waits.is_empty(), "it waits for the next swap");
    assert_no_resume(&d);
}

#[test]
fn issue_2236_a_full_snapshot_after_a_known_stop_is_no_room_and_waits() {
    let mut d = Fake::new();
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, -7, 100, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert!(
        d.outcomes().contains(&Outcome::SwapNoRoom),
        "{:?}",
        d.outcomes()
    );
    assert_no_resume(&d);
}

#[test]
fn issue_2236_the_wait_loop_judges_with_the_stop() {
    // No snapshot. The first read after arming finds a swap made between a
    // known stop and the late start: after the stop, so it resumes.
    let mut d = Fake::new();
    d.stop = Some(STOP);
    d.swap_race = Some(record_at(6, -7, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert!(d.waits.is_empty(), "no wait: {:?}", d.waits);
    // Without the stop the same record is stale and logged.
    let mut d = Fake::new();
    d.swap_race = Some(record_at(6, -7, 12, 40));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(
        d.outcomes(),
        [Outcome::SwapStale, Outcome::NoCapacityTimeout]
    );
}

#[test]
fn issue_2236_a_stop_never_makes_a_snapshot_in_the_slack_second_wait() {
    // D2: the stop only widens the window. A stop in the start second keeps
    // the one-second allowance, so this snapshot still resumes.
    let mut d = Fake::new();
    d.stop = Some(START);
    d.swap = Some(record_at(6, -1, 12, 40));
    assert_eq!(
        run_with(&limit_input("s", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

#[test]
fn issue_2236_the_stop_time_is_never_logged() {
    let mut d = Fake::new();
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, -7, 12, 40));
    run_with(&limit_input("s", "hit your limit"), &mut d);
    let shown = format!("{:?}", d.logs);
    assert!(!shown.contains(&STOP.to_string()), "{shown}");
}

// ── Issue #2235: the event key names the login the session stopped on ───────

#[test]
fn issue_2235_a_snapshot_after_the_stop_keys_the_event_on_its_outgoing_login() {
    // The rotator swapped work → spare between the stop and the hook's
    // start, so the ledger already shows spare. The session stopped on
    // work, and the event must say so.
    let mut d = Fake::new();
    d.cycle = Some(ledger("spare"));
    d.stop = Some(START - 5);
    d.swap = Some(record_at(6, -3, 12, 40));
    let msg = "You've hit your session limit";
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);
    let on_work = event_key("s", Some("rate_limit"), None, Some(msg), Some("work"));
    let on_spare = event_key("s", Some("rate_limit"), None, Some(msg), Some("spare"));
    assert!(d.guard.events.contains(&on_work), "{:?}", d.guard.events);
    assert!(!d.guard.events.contains(&on_spare));
    assert_eq!(d.last_log().event, on_work[..12]);
    assert_eq!(
        d.last_log().live.as_deref(),
        Some("spare"),
        "the log still names the ledger's live login"
    );
}

#[test]
fn issue_2235_a_limit_on_the_new_login_is_a_new_event_not_a_duplicate() {
    // Run 1 resumes the session on spare. It then hits a limit on spare
    // with the same text: a new stop, later than the swap, so it must wait
    // for the next swap rather than be dropped as a duplicate.
    let mut d = Fake::new();
    d.cycle = Some(ledger("spare"));
    d.stop = Some(START - 5);
    d.swap = Some(record_at(6, -3, 12, 40));
    let msg = "You've hit your session limit";
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);

    d.restart_at(START + 20);
    d.stop = Some(START + 15);
    let out = run_with(&limit_input("s", msg), &mut d);
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(d.guard.events.len(), 1, "no second resume");
    let on_spare = event_key("s", Some("rate_limit"), None, Some(msg), Some("spare"));
    assert_eq!(d.last_log().event, on_spare[..12]);
}

#[test]
fn issue_2235_without_a_stop_the_slack_snapshot_also_keys_on_its_outgoing_login() {
    // The snapshot in the slack second counts as after the stop, and the
    // key follows the same rule as the decision to resume.
    let mut d = Fake::new();
    d.cycle = Some(ledger("spare"));
    d.swap = Some(record_at(6, 0, 12, 40));
    let msg = "hit your limit";
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);
    let on_work = event_key("s", Some("rate_limit"), None, Some(msg), Some("work"));
    assert!(d.guard.events.contains(&on_work), "{:?}", d.guard.events);
}

#[test]
fn issue_2235_a_snapshot_before_the_stop_keys_on_the_live_login() {
    let mut d = Fake::new();
    d.cycle = Some(ledger("spare"));
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, -60, 12, 40));
    let msg = "hit your limit";
    assert_eq!(
        run_with(&limit_input("s", msg), &mut d),
        Outcome::NoCapacityTimeout
    );
    let on_spare = event_key("s", Some("rate_limit"), None, Some(msg), Some("spare"));
    assert_eq!(d.last_log().event, on_spare[..12]);
}

#[test]
fn issue_2235_a_snapshot_dated_after_the_start_keys_on_the_live_login() {
    // A clock ahead: the record was on disk before the hook started, so it
    // is not the swap the session stopped across.
    let mut d = Fake::new();
    d.cycle = Some(ledger("spare"));
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, 5, 12, 40));
    let msg = "hit your limit";
    assert_eq!(
        run_with(&limit_input("s", msg), &mut d),
        Outcome::NoCapacityTimeout
    );
    let on_spare = event_key("s", Some("rate_limit"), None, Some(msg), Some("spare"));
    assert_eq!(d.last_log().event, on_spare[..12]);
}

#[test]
fn issue_2235_a_second_hook_after_the_resume_never_resumes_twice() {
    // Once resumed on the swap, the swap is at or before the last resume,
    // so it is never after a later stop: the next hook keys on the live
    // login and waits for the next swap.
    let mut d = Fake::new();
    d.cycle = Some(ledger("spare"));
    d.stop = Some(START - 5);
    d.swap = Some(record_at(6, -3, 12, 40));
    let msg = "hit your limit";
    assert_eq!(run_with(&limit_input("s", msg), &mut d), Outcome::Resumed);
    // A second copy of the same hook, started in the same second.
    d.restart_at(START);
    assert_eq!(
        run_with(&limit_input("s", msg), &mut d),
        Outcome::NoCapacityTimeout
    );
    assert_eq!(d.guard.events.len(), 1, "one resume");
}

#[test]
fn issue_2235_stop_hook_active_still_touches_no_guard_state_with_a_snapshot() {
    let mut d = Fake::new();
    d.stop = Some(STOP);
    d.swap = Some(record_at(6, -7, 12, 40));
    let out = run_with(
        &input(json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "rate_limit",
            "stop_hook_active": true
        })),
        &mut d,
    );
    assert_eq!(out, Outcome::StopHookActive);
    assert_eq!(d.guard.calls, 0);
    assert_eq!(d.arms, 0);
}
