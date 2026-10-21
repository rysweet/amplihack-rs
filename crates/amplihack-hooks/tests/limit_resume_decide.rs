//! Issue #1658 (C): decision logic of `amplihack-hooks limit-resume`, the
//! `StopFailure` hook that wakes a session stopped by a usage limit once the
//! rotator has recorded a fresh cycle with capacity.
//!
//! Everything here runs against an in-memory guard and a fake clock: no real
//! waiting, no files. Filesystem behaviour is in `limit_resume_fs.rs`.
//!
//! Required tests from the issue:
//! (a) limit + auto-switch on + fresh swapped/stayed cycle → resume once
//! (b) no capacity / stale cycle / alert → no resume
//! (c) repeated distinct limit events → bounded by the per-session cap
//! (d) non-limit error or no match → no action
//! (e) auto-switch off → exit 0, no output, ledger never read
//! loop: 50 invocations on one event → at most 1 resume

use amplihack_hooks::limit_resume::decide::{
    Facts, Gate, MAX_MATCH_BYTES, event_key, fresh_capacity, gate, is_limit, session_id_ok,
    session_key,
};
use amplihack_hooks::limit_resume::guard::{CommitError, GuardStore};
use amplihack_hooks::limit_resume::log::LogEntry;
use amplihack_hooks::limit_resume::{
    Deps, MAX_RESUMES_PER_SESSION, Outcome, RESUME_MSG, run_with, run_with_io,
};
use amplihack_types::HookInput;
use amplihack_utils::rotator_status::{CycleStatus, FRESH_MARGIN_SECS, MAX_WAIT, POLL_INTERVAL};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Duration;

const START: i64 = 1_792_592_000;

// ── Test doubles ────────────────────────────────────────────────────────────

/// In-memory guard with the same contract as `FsGuard`.
#[derive(Default)]
struct MemGuard {
    events: HashSet<String>,
    sessions: HashMap<String, Vec<i64>>,
    fail_seen: bool,
    fail_resumes: bool,
    force_commit: Option<CommitError>,
    calls: u32,
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

    fn commit(&mut self, event: &str, session: &str, now: i64) -> Result<(), CommitError> {
        self.calls += 1;
        if let Some(e) = self.force_commit.take() {
            return Err(e);
        }
        if self.events.contains(event) {
            return Err(CommitError::Duplicate);
        }
        let v = self.sessions.entry(session.to_string()).or_default();
        if v.iter().filter(|t| now - **t < 86_400).count() as u32 >= MAX_RESUMES_PER_SESSION {
            return Err(CommitError::Cap);
        }
        self.events.insert(event.to_string());
        v.push(now);
        Ok(())
    }
}

type CycleFn = Box<dyn Fn(i64) -> Option<CycleStatus>>;

struct Fake {
    on: bool,
    clock: i64,
    /// What the ledger shows, given seconds since START.
    cycle: CycleFn,
    guard: MemGuard,
    reads: Vec<i64>,
    sleeps: Vec<Duration>,
    logs: Vec<LogEntry>,
    panic_on_now: bool,
}

impl Fake {
    fn new(on: bool, cycle: CycleFn) -> Self {
        Self {
            on,
            clock: START,
            cycle,
            guard: MemGuard::default(),
            reads: Vec::new(),
            sleeps: Vec::new(),
            logs: Vec::new(),
            panic_on_now: false,
        }
    }

    fn elapsed_reads(&self) -> Vec<i64> {
        self.reads.iter().map(|t| t - START).collect()
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
    fn sleep(&mut self, d: Duration) {
        self.sleeps.push(d);
        self.clock += d.as_secs() as i64;
    }
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus> {
        self.reads.push(now);
        (self.cycle)(now - START)
    }
    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }
    fn log(&mut self, _now: i64, entry: &LogEntry) {
        self.logs.push(entry.clone());
    }
}

fn cycle(outcome: &str, at_offset: i64, alert: bool) -> CycleStatus {
    CycleStatus {
        at: Some(START + at_offset),
        outcome: Some(outcome.into()),
        alert,
        live: Some("spare".into()),
    }
}

/// A ledger whose last cycle is recorded `after` seconds into the wait.
fn appears_at(outcome: &'static str, after: i64, alert: bool) -> CycleFn {
    Box::new(move |elapsed| {
        if elapsed >= after {
            Some(cycle(outcome, after, alert))
        } else {
            Some(cycle("stayed", -600, false))
        }
    })
}

fn never() -> CycleFn {
    Box::new(|_| None)
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

// ── Message and outcome strings ─────────────────────────────────────────────

#[test]
fn resume_message_is_the_documented_text() {
    assert_eq!(
        RESUME_MSG,
        "amplihack: the usage limit was cleared by switching to a login with capacity. \
         Continue the task you were working on."
    );
    assert!(!RESUME_MSG.contains('\n'));
}

#[test]
fn outcome_strings_and_exit_codes() {
    let table = [
        (Outcome::Resumed, "resumed", 2),
        (Outcome::NoCapacityTimeout, "no-capacity-timeout", 0),
        (Outcome::Duplicate, "duplicate", 0),
        (Outcome::Cap, "cap", 0),
        (Outcome::StopHookActive, "stop-hook-active", 0),
        (Outcome::GuardFailed, "guard-failed", 0),
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

/// error, error_details, last_assistant_message, expected.
type LimitCase = (
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    bool,
);

#[test]
fn is_limit_table() {
    let cases: &[LimitCase] = &[
        // error, error_details, last_assistant_message, expected
        (Some("rate_limit"), None, None, true),
        (
            Some("rate_limit"),
            Some("nothing useful"),
            Some("bye"),
            true,
        ),
        (
            None,
            None,
            Some("You've hit your weekly limit · resets 9am"),
            true,
        ),
        (None, None, Some("You've hit your session limit"), true),
        (None, None, Some("You've hit your usage limit"), true),
        (None, None, Some("You've hit your limit"), true),
        (
            None,
            None,
            Some("You've hit your org's monthly spend limit"),
            true,
        ),
        (None, Some("Claude AI usage: hit your limit"), None, true),
        (Some(""), None, Some("hit your limit"), true),
        (
            Some("unknown"),
            None,
            Some("Stop and wait for limit to reset"),
            true,
        ),
        (
            None,
            Some("1. Stop and wait for limit to reset\n2. Wait"),
            None,
            true,
        ),
        // A known non-limit error overrides the text.
        (
            Some("overloaded"),
            None,
            Some("You've hit your weekly limit"),
            false,
        ),
        (
            Some("authentication_failed"),
            Some("hit your limit"),
            None,
            false,
        ),
        (
            Some("server_error"),
            None,
            Some("Stop and wait for limit to reset"),
            false,
        ),
        (Some("RATE_LIMIT"), None, None, false),
        // No match.
        (None, None, Some("All done."), false),
        (None, None, None, false),
        (Some(""), Some(""), Some(""), false),
        (None, None, Some("limit"), false),
    ];
    for (e, d, m, want) in cases {
        assert_eq!(
            is_limit(*e, *d, *m),
            *want,
            "error={e:?} details={d:?} msg={m:?}"
        );
    }
}

#[test]
fn is_limit_matches_only_the_first_64_kib_of_each_field() {
    assert_eq!(MAX_MATCH_BYTES, 64 * 1024);
    let phrase = "hit your limit";
    let inside = format!("{}{phrase}", "x".repeat(MAX_MATCH_BYTES - phrase.len()));
    assert!(is_limit(None, None, Some(inside.as_str())));
    let outside = format!("{}{phrase}", "x".repeat(MAX_MATCH_BYTES));
    assert!(!is_limit(None, None, Some(outside.as_str())));
    assert!(!is_limit(None, Some(outside.as_str()), None));
}

#[test]
fn is_limit_does_not_panic_on_a_char_split_at_the_cap() {
    // 'é' is two bytes; an odd prefix puts the cap in the middle of one.
    let s = format!("x{}", "é".repeat(MAX_MATCH_BYTES));
    assert!(!is_limit(None, Some(s.as_str()), Some(s.as_str())));
}

// ── Keys ────────────────────────────────────────────────────────────────────

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[test]
fn event_key_is_stable_lowercase_sha256_hex() {
    let k = event_key("s", Some("rate_limit"), Some("d"), Some("m"));
    assert!(is_hex64(&k), "{k}");
    assert_eq!(k, event_key("s", Some("rate_limit"), Some("d"), Some("m")));
}

#[test]
fn event_key_changes_with_every_field() {
    let base = event_key("s", Some("rate_limit"), Some("d"), Some("m"));
    for other in [
        event_key("t", Some("rate_limit"), Some("d"), Some("m")),
        event_key("s", Some("rate_limited"), Some("d"), Some("m")),
        event_key("s", Some("rate_limit"), Some("e"), Some("m")),
        event_key("s", Some("rate_limit"), Some("d"), Some("n")),
    ] {
        assert_ne!(base, other);
    }
}

#[test]
fn event_key_fields_are_separated_unambiguously() {
    assert_ne!(
        event_key("ab", Some("c"), None, None),
        event_key("a", Some("bc"), None, None)
    );
    assert_ne!(
        event_key("s", None, Some("xy"), Some("z")),
        event_key("s", None, Some("x"), Some("yz"))
    );
}

#[test]
fn session_key_is_hex_and_hides_the_session_id() {
    let id = "session-ABC-123";
    let k = session_key(id);
    assert!(is_hex64(&k));
    assert!(!k.contains(id));
    assert_ne!(k, event_key(id, None, None, None));
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

// ── fresh_capacity ──────────────────────────────────────────────────────────

#[test]
fn fresh_capacity_needs_a_cycle_recorded_after_the_margin() {
    assert_eq!(FRESH_MARGIN_SECS, 60);
    let at = |o| Some(cycle("swapped", o, false));
    assert!(
        !fresh_capacity(at(-5).as_ref(), START),
        "older than the hook"
    );
    assert!(!fresh_capacity(at(0).as_ref(), START));
    assert!(!fresh_capacity(at(59).as_ref(), START));
    assert!(!fresh_capacity(at(60).as_ref(), START), "margin is strict");
    assert!(fresh_capacity(at(61).as_ref(), START));
    assert!(fresh_capacity(Some(&cycle("stayed", 61, false)), START));
}

#[test]
fn fresh_capacity_refuses_no_capacity_states() {
    assert!(!fresh_capacity(None, START));
    assert!(!fresh_capacity(Some(&cycle("swapped", 120, true)), START));
    for o in ["alerted", "skipped", "stopped", "idle", "mystery"] {
        assert!(!fresh_capacity(Some(&cycle(o, 120, false)), START), "{o}");
    }
    let no_at = CycleStatus {
        at: None,
        outcome: Some("swapped".into()),
        alert: false,
        live: None,
    };
    assert!(!fresh_capacity(Some(&no_at), START));
}

// ── (a) resume once ─────────────────────────────────────────────────────────

#[test]
fn a_limit_with_fresh_swapped_cycle_resumes_once() {
    let mut d = Fake::new(true, appears_at("swapped", 75, false));
    let out = run_with(&limit_input("s1", "You've hit your weekly limit"), &mut d);
    assert_eq!(out, Outcome::Resumed);
    assert_eq!(out.exit_code(), 2);
    assert_eq!(d.guard.events.len(), 1, "event marker committed");
    assert_eq!(d.guard.sessions.values().map(Vec::len).sum::<usize>(), 1);
    assert_eq!(d.logs.len(), 1);
    assert_eq!(d.logs[0].outcome, Outcome::Resumed);
    assert_eq!(d.logs[0].live.as_deref(), Some("spare"));
    assert!(d.clock - START < 75 + POLL_INTERVAL.as_secs() as i64 + 1);
}

#[test]
fn a_prime_stayed_cycle_also_resumes() {
    let mut d = Fake::new(true, appears_at("stayed", 90, false));
    assert_eq!(
        run_with(&limit_input("s1", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

#[test]
fn a_log_entry_holds_hash_prefixes_only() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
    run_with(&limit_input("secret-session-id", "msg"), &mut d);
    let e = &d.logs[0];
    assert_eq!(e.session, session_key("secret-session-id")[..12]);
    assert_eq!(
        e.event,
        event_key("secret-session-id", Some("rate_limit"), None, Some("msg"))[..12]
    );
}

// ── (b) no capacity → no resume ─────────────────────────────────────────────

fn assert_timeout(d: &Fake, out: Outcome) {
    assert_eq!(out, Outcome::NoCapacityTimeout);
    assert_eq!(out.exit_code(), 0);
    assert!(d.guard.events.is_empty(), "no marker without a resume");
    let total: Duration = d.sleeps.iter().sum();
    assert!(total <= MAX_WAIT, "slept {total:?}");
    assert_eq!(
        d.elapsed_reads().last().copied(),
        Some(MAX_WAIT.as_secs() as i64),
        "one final read exactly at the deadline"
    );
    for s in &d.sleeps {
        assert!(*s <= POLL_INTERVAL && !s.is_zero(), "sleep {s:?}");
    }
    assert_eq!(d.logs.len(), 1);
    assert_eq!(d.logs[0].outcome, Outcome::NoCapacityTimeout);
}

#[test]
fn b1_alerted_cycle_never_resumes() {
    let mut d = Fake::new(true, appears_at("alerted", 30, false));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_timeout(&d, out);
}

#[test]
fn b2_alert_set_never_resumes_even_if_swapped() {
    let mut d = Fake::new(true, appears_at("swapped", 90, true));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_timeout(&d, out);
}

#[test]
fn b3_cycle_older_than_the_hook_never_resumes() {
    let mut d = Fake::new(true, Box::new(|_| Some(cycle("swapped", -5, false))));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_timeout(&d, out);
}

#[test]
fn b4_cycle_inside_the_freshness_margin_never_resumes() {
    let mut d = Fake::new(true, Box::new(|_| Some(cycle("swapped", 59, false))));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_timeout(&d, out);
}

#[test]
fn b5_missing_ledger_never_resumes() {
    let mut d = Fake::new(true, never());
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_timeout(&d, out);
}

#[test]
fn b6_idle_and_unknown_outcomes_never_resume() {
    for o in ["idle", "skipped", "stopped", "something-new"] {
        let mut d = Fake::new(true, appears_at(o, 61, false));
        let out = run_with(&limit_input("s", "hit your limit"), &mut d);
        assert_timeout(&d, out);
    }
}

#[test]
fn capacity_exactly_at_the_deadline_is_seen_by_the_final_read() {
    let deadline = MAX_WAIT.as_secs() as i64;
    let mut d = Fake::new(true, appears_at("swapped", deadline, false));
    assert_eq!(
        run_with(&limit_input("s", "hit your limit"), &mut d),
        Outcome::Resumed
    );
}

#[test]
fn capacity_after_the_deadline_is_not_waited_for() {
    let deadline = MAX_WAIT.as_secs() as i64;
    let mut d = Fake::new(true, appears_at("swapped", deadline + 1, false));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_timeout(&d, out);
}

// ── (c) repeated distinct events are capped per session ─────────────────────

#[test]
fn c_distinct_limit_events_stop_at_the_session_cap() {
    let mut d = Fake::new(true, never());
    let mut resumed = 0;
    for i in 0..6 {
        // Each event: a fresh cycle 61 s after this hook's start.
        let t0 = d.clock - START;
        d.cycle = appears_at("swapped", t0 + 61, false);
        let out = run_with(&limit_input("s1", &format!("hit your limit #{i}")), &mut d);
        if out == Outcome::Resumed {
            resumed += 1;
        } else {
            assert_eq!(out, Outcome::Cap, "event {i}");
        }
    }
    assert_eq!(resumed, MAX_RESUMES_PER_SESSION);
}

#[test]
fn c_capped_session_exits_without_waiting() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
    d.guard
        .sessions
        .insert(session_key("s1"), vec![START - 10, START - 20, START - 30]);
    let out = run_with(&limit_input("s1", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Cap);
    assert!(
        d.reads.is_empty(),
        "a capped session must not poll the ledger"
    );
    assert!(d.sleeps.is_empty());
    assert_eq!(d.logs[0].outcome, Outcome::Cap);
}

#[test]
fn c_other_sessions_are_not_capped_by_this_one() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
    assert!(d.sleeps.is_empty(), "slept");
    assert_eq!(d.guard.calls, 0, "guard touched");
    assert!(d.logs.is_empty(), "logged");
}

#[test]
fn d_non_limit_error_is_ignored_even_with_matching_text() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
fn d_prime_no_error_and_no_match_is_ignored() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
        let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
        let mut d = Fake::new(true, appears_at("swapped", 61, false));
        assert_eq!(run_with(&input(v), &mut d), Outcome::NotLimit);
        assert_untouched(&d);
    }
}

// ── (e) auto-switch off ─────────────────────────────────────────────────────

#[test]
fn e_auto_switch_off_does_nothing() {
    let mut d = Fake::new(false, appears_at("swapped", 0, false));
    let out = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(out, Outcome::Disabled);
    assert_eq!(out.exit_code(), 0);
    assert_untouched(&d);
}

#[test]
fn e_auto_switch_off_prints_nothing() {
    let mut d = Fake::new(false, appears_at("swapped", 0, false));
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit"
    })
    .to_string();
    let mut stderr = Vec::new();
    let code = run_with_io(payload.as_bytes(), &mut stderr, &mut d);
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert_untouched(&d);
}

// ── stop_hook_active, duplicate, guard failures ─────────────────────────────

#[test]
fn stop_hook_active_never_resumes_and_touches_no_guard_state() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
    assert!(d.reads.is_empty());
    assert_eq!(d.logs.len(), 1);
    assert_eq!(d.logs[0].outcome, Outcome::StopHookActive);
}

#[test]
fn duplicate_event_exits_without_waiting() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
    let first = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(first, Outcome::Resumed);
    let reads_before = d.reads.len();
    let second = run_with(&limit_input("s", "hit your limit"), &mut d);
    assert_eq!(second, Outcome::Duplicate);
    assert_eq!(d.reads.len(), reads_before, "duplicate must not poll");
}

#[test]
fn guard_failure_before_waiting_is_guard_failed() {
    for which in ["seen", "resumes"] {
        let mut d = Fake::new(true, appears_at("swapped", 61, false));
        match which {
            "seen" => d.guard.fail_seen = true,
            _ => d.guard.fail_resumes = true,
        }
        let out = run_with(&limit_input("s", "hit your limit"), &mut d);
        assert_eq!(out, Outcome::GuardFailed, "{which}");
        assert_eq!(out.exit_code(), 0);
        assert!(
            d.reads.is_empty(),
            "{which}: must not poll after a guard failure"
        );
    }
}

#[test]
fn losing_the_commit_race_never_resumes() {
    for (err, want) in [
        (CommitError::Duplicate, Outcome::Duplicate),
        (CommitError::Cap, Outcome::Cap),
        (CommitError::Io, Outcome::GuardFailed),
    ] {
        let mut d = Fake::new(true, appears_at("swapped", 61, false));
        d.guard.force_commit = Some(err);
        let out = run_with(&limit_input("s", "hit your limit"), &mut d);
        assert_eq!(out, want);
        assert_eq!(out.exit_code(), 0);
        assert_eq!(d.logs.last().unwrap().outcome, want);
    }
}

// ── No infinite continue loop ───────────────────────────────────────────────

#[test]
fn loop_fifty_invocations_on_one_event_resume_at_most_once() {
    let mut d = Fake::new(true, never());
    let mut resumed = 0;
    for _ in 0..50 {
        let t0 = d.clock - START;
        d.cycle = appears_at("swapped", t0 + 61, false);
        if run_with(&limit_input("s", "You've hit your weekly limit"), &mut d) == Outcome::Resumed {
            resumed += 1;
        }
    }
    assert_eq!(resumed, 1);
}

#[test]
fn loop_stop_hook_active_invocations_never_resume() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
    }
    assert!(d.guard.events.is_empty());
}

#[test]
fn loop_resume_then_same_limit_again_is_not_resumed() {
    // The model is woken, the next request hits the same limit with the same
    // text: same key, so no second wake.
    let mut d = Fake::new(true, never());
    let mut outcomes = Vec::new();
    for _ in 0..5 {
        let t0 = d.clock - START;
        d.cycle = appears_at("stayed", t0 + 61, false);
        outcomes.push(run_with(
            &limit_input("s", "You've hit your session limit"),
            &mut d,
        ));
    }
    assert_eq!(outcomes[0], Outcome::Resumed);
    assert!(outcomes[1..].iter().all(|o| *o == Outcome::Duplicate));
}

// ── run_with_io: stdin/stderr protocol ──────────────────────────────────────

#[test]
fn run_with_io_resume_writes_exactly_the_message_to_stderr() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
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
        let mut d = Fake::new(true, appears_at("swapped", 61, false));
        let mut stderr = Vec::new();
        assert_eq!(run_with_io(payload.as_bytes(), &mut stderr, &mut d), 0);
        assert!(stderr.is_empty(), "{payload:?}");
        assert_untouched(&d);
    }
}

#[test]
fn run_with_io_refuses_more_than_one_mib_of_stdin() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
    let mut payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "error_details": ""
    })
    .to_string();
    // Pad inside the JSON so the payload would otherwise be valid.
    let pad = "x".repeat(1024 * 1024);
    payload = payload.replace(
        "\"error_details\":\"\"",
        &format!("\"error_details\":\"{pad}\""),
    );
    let mut stderr = Vec::new();
    assert_eq!(run_with_io(payload.as_bytes(), &mut stderr, &mut d), 0);
    assert!(stderr.is_empty());
    assert_untouched(&d);
}

#[test]
fn run_with_io_turns_a_panic_into_a_silent_exit_zero() {
    let mut d = Fake::new(true, appears_at("swapped", 61, false));
    d.panic_on_now = true;
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit"
    })
    .to_string();
    let mut stderr = Vec::new();
    assert_eq!(run_with_io(payload.as_bytes(), &mut stderr, &mut d), 0);
    assert!(stderr.is_empty());
}
