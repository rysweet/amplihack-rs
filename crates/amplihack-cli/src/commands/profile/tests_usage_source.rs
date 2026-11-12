//! `usage_source` (issue #1844): the order in which a row is read, and the
//! lazy `claude` lookup.
//!
//! Contract:
//! - `Source` is `Passive`, `Direct` or `Fallback`; `as_str()` gives
//!   `passive`, `direct` and `fallback`, and it serialises the same way.
//! - `SourceEnv { mode, passive_max_age, endpoints, timeout }` is resolved
//!   once per command or cycle.
//! - `Plan { creds_dir, passive, allow_refresh, overflow }`: `passive` names
//!   the profile directory whose stored reading may be used (`None` for the
//!   `~/.claude` row); `overflow` is set for a profile marked as overflow.
//! - `read(env, plan, root, now, fallback) -> Probe`. `fallback(mode)` runs
//!   the `claude` probe with `mode` (`Auto` or `Text`) and is called at most
//!   once, only when the order needs it:
//!
//!   | Mode / profile | Steps | Fallback mode |
//!   |---|---|---|
//!   | `Auto` / `Text` | fallback only | as named |
//!   | overflow, not `Direct` | fallback only | `Text` |
//!   | overflow, `Direct` | none: error `direct read: overflow needs /usage` | - |
//!   | `Ordered` | passive, direct, fallback | `Auto`; `Text` when credits on |
//!   | `Direct` | passive, direct; a failure is error `direct read: <reason>` | - |
//!
//! - `Probe::source` is the step that gave the row, and `Probe::direct_failure`
//!   the fixed reason of a direct read that led to a fallback.
//! - Issue #1856: credits on with a readable window is a `direct` row with
//!   the direct windows, `extra_shape: Some(..)` and spend `NotProbed`, in
//!   both `Ordered` and `Direct`, and no fallback is called. Only credits on
//!   with neither window falls back (`Text`, `usage credits on`).
//! - `Spend` is `NotApplicable`, `Ok`, `LimitReached`, `Unknown` or
//!   `NotProbed`; `as_str()` gives `n/a`, `ok`, `limit_reached`, `unknown`
//!   and `not_probed`: a probe that failed and one never asked for differ. `Spend::of_text(&Probe)` is `Unknown` for any error and
//!   for the states `error`, `timeout`, `not_logged_in` and `unparsed`,
//!   `LimitReached` for `spend_limit`, and `Ok` otherwise: `ok` is never
//!   guessed.
//! - A fallback row keeps the spend its probe gave (the `claude` probe
//!   knows whether `get_usage` or `/usage` answered); `read` never changes
//!   it. `Probe::read` gives `NotApplicable`, `Probe::failed` and
//!   `Probe::direct_error` give `Unknown`, and none of them has a shape.
//! - `LazyBinary::new(resolve)` calls `resolve` at most once, on the first
//!   `get()`. `fallback_with(&LazyBinary, run)` gives an error row
//!   (`claude not found`) when no binary is found and calls `run` otherwise.
//!
//! The direct reads go to a loopback fake; `fallback` is a closure that
//! records its calls. No `claude` is started.

use super::name::ProfileName;
use super::probe_limits::ProbeMode;
use super::tests_fake_http::{FakeHttp, Reply};
use super::usage_cmd::{ErrorReason, Probe};
use super::usage_direct::Endpoints;
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use super::usage_source::{LazyBinary, Plan, Source, SourceEnv, Spend, fallback_with, read};
use crate::binary_finder::BinaryInfo;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const ACCESS: &str = "FAKE-ACCESS-1844-src";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn iso(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn body(session: f64, week: f64, credits: bool) -> Value {
    let n = now();
    json!({
        "five_hour": {"utilization": session, "resets_at": iso(n + 3600)},
        "seven_day": {"utilization": week, "resets_at": iso(n + 3 * 86_400)},
        "extra_usage": {"is_enabled": credits}
    })
}

struct Setup {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Setup {
    fn dir(&self) -> PathBuf {
        self.root.join("spare")
    }

    /// A fresh stored reading for `spare`: 7% and 9%.
    fn seed_passive(&self) {
        let state = self.root.join(".auto-switch.d");
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let n = now();
        let usage = json!({"version": 1, "accounts": {"spare": {
            "five_hour": {"used_percentage": 7.0, "resets_at": n + 3600},
            "seven_day": {"used_percentage": 9.0, "resets_at": n + 86_400},
            "observed_at": n - 5,
            "source": "statusline"
        }}});
        let p = state.join("usage.json");
        fs::write(&p, usage.to_string()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn plan(&self) -> Plan {
        Plan {
            creds_dir: self.dir(),
            passive: Some(ProfileName::parse("spare").unwrap()),
            allow_refresh: true,
            overflow: false,
        }
    }
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("profiles");
    fs::create_dir_all(root.join("spare")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("spare"), fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let creds = root.join("spare").join(".credentials.json");
    let doc = json!({"claudeAiOauth": {
        "accessToken": ACCESS,
        "refreshToken": "FAKE-REFRESH-1844-src",
        "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000
    }});
    fs::write(&creds, doc.to_string()).unwrap();
    fs::set_permissions(&creds, fs::Permissions::from_mode(0o600)).unwrap();
    Setup { _tmp: tmp, root }
}

fn env_for(fake: &FakeHttp, mode: ProbeMode) -> SourceEnv {
    let base = fake.base();
    let token = fake.token_url();
    let (endpoints, _) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&token)));
    SourceEnv {
        mode,
        passive_max_age: 300,
        endpoints,
        timeout: Duration::from_secs(5),
    }
}

/// The `claude` probe stand-in: records the mode and gives 33%/44% with
/// the spend it was built with (`ok`, as `/usage` text would give).
struct Fallback {
    calls: RefCell<Vec<ProbeMode>>,
    state: UsageState,
    spend: Spend,
}

impl Fallback {
    fn new() -> Self {
        Self::giving(UsageState::Ok, Spend::Ok)
    }

    fn giving(state: UsageState, spend: Spend) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            state,
            spend,
        }
    }

    fn run(&self, mode: ProbeMode) -> Probe {
        self.calls.borrow_mut().push(mode);
        let w = |label: &str, p: f64| UsageWindow {
            label: label.into(),
            used_percentage: p,
            resets_text: None,
            resets_at: None,
        };
        Probe {
            report: UsageReport {
                state: self.state,
                session: Some(w("session", 33.0)),
                week_all: Some(w("all models", 44.0)),
                week_models: Vec::new(),
                session_limit: true,
                observed_at: None,
            },
            error: None,
            raw: None,
            source: Some(Source::Fallback),
            direct_failure: None,
            spend: self.spend,
            extra_shape: None,
        }
    }

    fn calls(&self) -> Vec<ProbeMode> {
        self.calls.borrow().clone()
    }
}

fn session(p: &Probe) -> Option<f64> {
    p.report.session.as_ref().map(|w| w.used_percentage)
}

fn week(p: &Probe) -> Option<f64> {
    p.report.week_all.as_ref().map(|w| w.used_percentage)
}

fn ok_server() -> FakeHttp {
    FakeHttp::start(|_, _| Reply::json(200, &body(100.0, 55.0, false)))
}

fn failing_server() -> FakeHttp {
    FakeHttp::start(|_, _| Reply::raw(500, ""))
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

#[test]
fn sources_have_the_documented_names() {
    assert_eq!(Source::Passive.as_str(), "passive");
    assert_eq!(Source::Direct.as_str(), "direct");
    assert_eq!(Source::Fallback.as_str(), "fallback");
    assert_eq!(serde_json::to_value(Source::Fallback).unwrap(), "fallback");
}

// ---------------------------------------------------------------------------
// Ordered (the default)
// ---------------------------------------------------------------------------

#[test]
fn ordered_uses_a_fresh_passive_reading_and_asks_nothing() {
    let s = setup();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Passive));
    assert_eq!((session(&p), week(&p)), (Some(7.0), Some(9.0)));
    assert!(fake.seen().is_empty());
    assert!(fb.calls().is_empty());
}

#[test]
fn ordered_reads_directly_when_there_is_no_fresh_reading() {
    let s = setup();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Direct));
    assert_eq!(p.report.state, UsageState::SessionExhausted);
    assert_eq!((session(&p), week(&p)), (Some(100.0), Some(55.0)));
    assert_eq!(p.error, None);
    assert_eq!(p.raw, None);
    assert_eq!(fake.seen().len(), 1);
    assert!(fb.calls().is_empty(), "no claude for a direct row");
}

#[test]
fn passive_age_zero_reads_directly() {
    let s = setup();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut env = env_for(&fake, ProbeMode::Ordered);
    env.passive_max_age = 0;
    let p = read(&env, &s.plan(), &s.root, now(), |m| fb.run(m));
    assert_eq!(p.source, Some(Source::Direct));
}

#[test]
fn ordered_falls_back_to_the_auto_probe_when_the_direct_read_fails() {
    let s = setup();
    let fake = failing_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Auto]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert_eq!(p.direct_failure, Some("http 5xx"));
    assert_eq!((session(&p), week(&p)), (Some(33.0), Some(44.0)));
}

/// Issue #1856 (C2): credits on with neither window readable still falls
/// back to `/usage` text.
#[test]
fn ordered_falls_back_to_text_when_credits_are_on_and_no_window_is_read() {
    let s = setup();
    let fake = FakeHttp::start(|_, _| {
        Reply::json(
            200,
            &json!({"five_hour": null, "extra_usage": {"is_enabled": true}}),
        )
    });
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Text]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert_eq!(p.direct_failure, Some("usage credits on"));
}

/// Issue #2271 (D1): on Linux the file is the login, so a profile directory
/// with no login file is final. `claude` would find the same empty
/// directory, so none is started.
#[cfg(target_os = "linux")]
#[test]
fn ordered_gives_not_signed_in_when_there_is_no_login_file() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty(), "no claude: {:?}", fb.calls());
    assert_not_signed_in(&p, "no credentials file");
    assert!(fake.seen().is_empty());
}

/// Issue #2271 (D1): off Linux the login may live in the Keychain, so a
/// missing file is not a sign that the profile is signed out. It falls
/// back to `claude`, as before.
#[cfg(not(target_os = "linux"))]
#[test]
fn ordered_falls_back_when_there_is_no_login_file() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Auto]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert_eq!(p.direct_failure, Some("no credentials file"));
    assert!(p.error.is_none(), "{:?}", p.error);
}

#[test]
fn the_home_row_has_no_passive_step() {
    let s = setup();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.passive = None;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Direct));
}

#[test]
fn no_refresh_is_passed_through_and_a_401_falls_back() {
    let s = setup();
    let fake = FakeHttp::start(|s, _| {
        if s.method == "POST" {
            Reply::json(
                200,
                &json!({"access_token": "FAKE-NEW", "expires_in": 3600}),
            )
        } else {
            Reply::json(401, &json!({}))
        }
    });
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.allow_refresh = false;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fake.count("POST", "/v1/oauth/token"), 0);
    assert_eq!(p.direct_failure, Some("http 401"));
    assert_eq!(p.source, Some(Source::Fallback));
}

#[test]
fn an_overflow_profile_gets_the_text_probe_only() {
    let s = setup();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.overflow = true;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Text]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert!(fake.seen().is_empty(), "never read directly");
}

// ---------------------------------------------------------------------------
// Named modes
// ---------------------------------------------------------------------------

#[test]
fn auto_and_text_use_the_named_claude_probe_only() {
    for mode in [ProbeMode::Auto, ProbeMode::Text] {
        let s = setup();
        s.seed_passive();
        let fake = ok_server();
        let fb = Fallback::new();
        let p = read(&env_for(&fake, mode), &s.plan(), &s.root, now(), |m| {
            fb.run(m)
        });
        assert_eq!(fb.calls(), [mode]);
        assert_eq!(p.source, Some(Source::Fallback));
        assert!(fake.seen().is_empty(), "{mode:?}");
    }
}

#[test]
fn direct_mode_reads_passive_then_direct() {
    let s = setup();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Passive));
    let s = setup();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Direct));
    assert!(fb.calls().is_empty());
}

#[test]
fn direct_mode_reports_a_failure_as_an_error_and_starts_no_claude() {
    let s = setup();
    let fake = failing_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_eq!(p.report.state, UsageState::Error);
    assert_eq!(p.source, Some(Source::Direct));
    let e = p.error.expect("an error reason");
    assert_eq!(e, ErrorReason::Direct("http 5xx"));
    assert_eq!(e.to_string(), "direct read: http 5xx");
}

#[test]
fn direct_mode_does_not_read_an_overflow_profile() {
    let s = setup();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.overflow = true;
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert!(fake.seen().is_empty());
    assert_eq!(p.report.state, UsageState::Error);
    assert_eq!(
        p.error.map(|e| e.to_string()).as_deref(),
        Some("direct read: overflow needs /usage")
    );
}

// ---------------------------------------------------------------------------
// The lazy `claude` lookup
// ---------------------------------------------------------------------------

fn fake_binary() -> BinaryInfo {
    BinaryInfo {
        name: "claude".into(),
        path: PathBuf::from("/nonexistent/claude"),
        version: None,
    }
}

#[test]
fn the_binary_is_resolved_once_and_only_when_asked() {
    let n = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&n);
    let lazy = LazyBinary::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Some(fake_binary())
    });
    assert_eq!(n.load(Ordering::SeqCst), 0, "not resolved at creation");
    let clone = lazy.clone();
    assert!(lazy.get().is_some());
    assert!(clone.get().is_some());
    assert_eq!(n.load(Ordering::SeqCst), 1, "clones share one lookup");
}

#[test]
fn rows_read_without_claude_never_resolve_the_binary() {
    let n = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&n);
    let lazy = LazyBinary::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Some(fake_binary())
    });
    let s = setup();
    let fake = ok_server();
    let env = env_for(&fake, ProbeMode::Ordered);
    for _ in 0..3 {
        let p = read(&env, &s.plan(), &s.root, now(), |m| {
            fallback_with(&lazy, |_bin| Fallback::new().run(m))
        });
        assert_eq!(p.source, Some(Source::Direct));
    }
    assert_eq!(n.load(Ordering::SeqCst), 0);
}

#[test]
fn no_claude_fails_only_the_row_that_needed_it() {
    let lazy = LazyBinary::new(|| None);
    let mut ran = false;
    let p = fallback_with(&lazy, |_| {
        ran = true;
        Fallback::new().run(ProbeMode::Auto)
    });
    assert!(!ran);
    assert_eq!(p.report.state, UsageState::Error);
    assert_eq!(p.error, Some(ErrorReason::NoClaude));
    assert_eq!(ErrorReason::NoClaude.to_string(), "claude not found");
    assert_eq!(p.source, Some(Source::Fallback));
}

#[test]
fn error_reasons_keep_their_fixed_texts() {
    assert_eq!(
        ErrorReason::ProfileRefused.to_string(),
        "profile directory refused"
    );
    assert_eq!(
        ErrorReason::StartFailed.to_string(),
        "could not start claude"
    );
    assert_eq!(
        ErrorReason::Direct("timeout").to_string(),
        "direct read: timeout"
    );
}

// ---------------------------------------------------------------------------
// Issue #1856: credits on, and the spend status
// ---------------------------------------------------------------------------

use super::usage_direct::Direct;
use super::usage_request::ExtraUsageShape;

/// Credits on with sentinel values that must never reach the row.
fn credits_on_body(session: f64, week: f64) -> Value {
    let mut b = body(session, week, true);
    b["extra_usage"] = json!({
        "is_enabled": true,
        "monthly_limit": 987_654,
        "used_credits": 4321.5,
        "currency": "SENTINELCUR",
        "email": "SENTINEL-EMAIL@example.invalid"
    });
    b
}

#[test]
fn spend_values_have_the_documented_names() {
    assert_eq!(Spend::NotApplicable.as_str(), "n/a");
    assert_eq!(Spend::Ok.as_str(), "ok");
    assert_eq!(Spend::LimitReached.as_str(), "limit_reached");
    assert_eq!(Spend::Unknown.as_str(), "unknown");
    assert_eq!(Spend::NotProbed.as_str(), "not_probed");
}

fn text_probe(state: UsageState) -> Probe {
    let mut p = Probe::read(UsageReport::empty(state), Source::Fallback);
    p.spend = Spend::Unknown;
    p
}

#[test]
fn of_text_reads_only_a_usage_reading_as_ok() {
    for (state, want) in [
        (UsageState::Ok, Spend::Ok),
        (UsageState::SessionExhausted, Spend::Ok),
        (UsageState::WeeklyExhausted, Spend::Ok),
        (UsageState::SpendLimit, Spend::LimitReached),
        (UsageState::NotLoggedIn, Spend::Unknown),
        (UsageState::Unparsed, Spend::Unknown),
        (UsageState::Timeout, Spend::Unknown),
        (UsageState::Error, Spend::Unknown),
    ] {
        assert_eq!(Spend::of_text(&text_probe(state)), want, "{state:?}");
    }
}

#[test]
fn of_text_never_gives_ok_for_a_failed_probe() {
    for state in [UsageState::Ok, UsageState::SpendLimit] {
        let mut p = text_probe(state);
        p.error = Some(ErrorReason::StartFailed);
        assert_eq!(Spend::of_text(&p), Spend::Unknown, "{state:?}");
    }
    let failed = Probe::failed(ErrorReason::WaitFailed, None);
    assert_eq!(Spend::of_text(&failed), Spend::Unknown);
}

#[test]
fn probe_constructors_set_spend_and_no_shape() {
    let read_p = Probe::read(UsageReport::empty(UsageState::Ok), Source::Direct);
    assert_eq!(read_p.spend, Spend::NotApplicable);
    assert_eq!(read_p.extra_shape, None);
    let failed = Probe::failed(ErrorReason::ReadFailed, None);
    assert_eq!(failed.spend, Spend::Unknown);
    assert_eq!(failed.extra_shape, None);
    let err = Probe::direct_error("http 5xx");
    assert_eq!(err.spend, Spend::Unknown);
    assert_eq!(err.extra_shape, None);
}

#[test]
fn probe_direct_marks_a_credits_on_row_as_not_probed() {
    let report = UsageReport::empty(UsageState::Ok);
    let off = Probe::direct(Direct {
        report: report.clone(),
        credits_on: None,
    });
    assert_eq!(off.source, Some(Source::Direct));
    assert_eq!(off.spend, Spend::NotApplicable);
    assert_eq!(off.extra_shape, None);
    let shape = ExtraUsageShape::of(&json!({"is_enabled": true}));
    let on = Probe::direct(Direct {
        report: report.clone(),
        credits_on: Some(shape.clone()),
    });
    assert_eq!(on.source, Some(Source::Direct));
    assert_eq!(on.spend, Spend::NotProbed);
    assert_eq!(on.extra_shape, Some(shape));
    assert_eq!(on.report, report);
    assert_eq!(on.error, None);
    assert_eq!(on.direct_failure, None);
}

#[test]
fn ordered_reads_a_credits_on_profile_directly_with_its_windows() {
    let s = setup();
    let fake = FakeHttp::start(|_, _| Reply::json(200, &credits_on_body(12.0, 34.0)));
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty(), "the windows need no claude");
    assert_eq!(p.source, Some(Source::Direct));
    assert_eq!((session(&p), week(&p)), (Some(12.0), Some(34.0)));
    assert_eq!(p.report.state, UsageState::Ok);
    assert_eq!(p.error, None);
    assert_eq!(p.direct_failure, None);
    assert_eq!(p.spend, Spend::NotProbed, "phase 2 decides the spend");
    let shape = p.extra_shape.as_ref().expect("the shape is kept for --raw");
    for leak in ["SENTINEL", "987654", "4321.5", "@"] {
        assert!(!shape.line().contains(leak), "{leak}: {}", shape.line());
        assert!(!format!("{p:?}").contains(leak), "{leak} in the probe");
    }
}

#[test]
fn ordered_keeps_an_exhausted_credits_on_state_from_the_windows() {
    let s = setup();
    let fake = FakeHttp::start(|_, _| Reply::json(200, &credits_on_body(100.0, 55.0)));
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.report.state, UsageState::SessionExhausted);
    assert_eq!(week(&p), Some(55.0));
    assert!(fb.calls().is_empty());
}

#[test]
fn direct_mode_gives_credits_on_windows_instead_of_an_error() {
    let s = setup();
    let fake = FakeHttp::start(|_, _| Reply::json(200, &credits_on_body(12.0, 34.0)));
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_eq!(p.source, Some(Source::Direct));
    assert_eq!(p.error, None);
    assert_eq!((session(&p), week(&p)), (Some(12.0), Some(34.0)));
    assert_eq!(p.spend, Spend::NotProbed);
    assert!(p.extra_shape.is_some());
}

#[test]
fn direct_mode_with_credits_on_and_no_window_is_still_an_error() {
    let s = setup();
    let fake = FakeHttp::start(|_, _| {
        Reply::json(
            200,
            &json!({"five_hour": null, "extra_usage": {"is_enabled": true}}),
        )
    });
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_eq!(p.error, Some(ErrorReason::Direct("usage credits on")));
    assert_eq!(p.spend, Spend::Unknown);
    assert_eq!(p.extra_shape, None);
}

#[test]
fn a_credits_off_direct_row_has_no_spend_to_read() {
    let s = setup();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Direct));
    assert_eq!(p.spend, Spend::NotApplicable);
    assert_eq!(p.extra_shape, None);
}

#[test]
fn a_passive_row_has_no_spend_to_read() {
    let s = setup();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Passive));
    assert_eq!(p.spend, Spend::NotApplicable);
}

#[test]
fn a_fallback_row_keeps_the_spend_its_probe_gave() {
    for (state, spend) in [
        (UsageState::SpendLimit, Spend::LimitReached),
        (UsageState::Ok, Spend::Ok),
        // An accepted `get_usage` answer: credits are off.
        (UsageState::Ok, Spend::NotApplicable),
        (UsageState::NotLoggedIn, Spend::Unknown),
    ] {
        let s = setup();
        let fake = failing_server();
        let fb = Fallback::giving(state, spend);
        let p = read(
            &env_for(&fake, ProbeMode::Ordered),
            &s.plan(),
            &s.root,
            now(),
            |m| fb.run(m),
        );
        assert_eq!(p.source, Some(Source::Fallback));
        assert_eq!(p.spend, spend, "{state:?}");
        assert_eq!(p.extra_shape, None);
    }
}

#[test]
fn an_overflow_row_keeps_its_text_spend() {
    let s = setup();
    let fake = ok_server();
    let fb = Fallback::giving(UsageState::SpendLimit, Spend::LimitReached);
    let mut plan = s.plan();
    plan.overflow = true;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Text]);
    assert_eq!(p.spend, Spend::LimitReached);
}

#[test]
fn error_rows_have_an_unknown_spend() {
    let s = setup();
    let fake = failing_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.spend, Spend::Unknown);
    let lazy = LazyBinary::new(|| None);
    let p = fallback_with(&lazy, |_| Fallback::new().run(ProbeMode::Auto));
    assert_eq!(p.spend, Spend::Unknown, "claude not found");
}

// ---------------------------------------------------------------------------
// Issue #1974 (AC5): fresh passive readings start no probe; one stale
// reading starts one fallback, for that login only.
// ---------------------------------------------------------------------------

/// Two profiles `a` and `b` under one root, each with a stored reading
/// observed `ages[i]` seconds ago.
fn two_logins(ages: [i64; 2]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("profiles");
    for p in ["a", "b"] {
        fs::create_dir_all(root.join(p)).unwrap();
        fs::set_permissions(root.join(p), fs::Permissions::from_mode(0o700)).unwrap();
        // A login, so that a stale reading reaches the direct read and its
        // failure falls back (issue #2271: no login file reads "not signed
        // in" and starts no probe).
        let creds = root.join(p).join(".credentials.json");
        let doc = json!({"claudeAiOauth": {
            "accessToken": ACCESS,
            "refreshToken": "FAKE-REFRESH-1974-src",
            "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000
        }});
        fs::write(&creds, doc.to_string()).unwrap();
        fs::set_permissions(&creds, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let state = root.join(".auto-switch.d");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let n = now();
    let entry = |age: i64| {
        json!({
            "five_hour": {"used_percentage": 7.0, "resets_at": n + 3600},
            "seven_day": {"used_percentage": 9.0, "resets_at": n + 86_400},
            "observed_at": n - age,
            "source": "statusline"
        })
    };
    let usage = json!({"version": 1, "accounts": {"a": entry(ages[0]), "b": entry(ages[1])}});
    let p = state.join("usage.json");
    fs::write(&p, usage.to_string()).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    Setup { _tmp: tmp, root }
}

fn plan_for(root: &std::path::Path, name: &str) -> Plan {
    Plan {
        creds_dir: root.join(name),
        passive: Some(ProfileName::parse(name).unwrap()),
        allow_refresh: true,
        overflow: false,
    }
}

#[test]
fn fresh_passive_readings_start_no_probe_for_any_login() {
    let s = two_logins([5, 60]);
    let fake = failing_server();
    let env = env_for(&fake, ProbeMode::Ordered);
    let mut probed = Vec::new();
    for name in ["a", "b"] {
        let fb = Fallback::new();
        let p = read(&env, &plan_for(&s.root, name), &s.root, now(), |m| {
            fb.run(m)
        });
        assert_eq!(p.source, Some(Source::Passive), "{name}");
        if !fb.calls().is_empty() {
            probed.push(name);
        }
    }
    assert!(probed.is_empty(), "probed {probed:?}");
    assert!(fake.seen().is_empty(), "no direct read either");
}

#[test]
fn one_stale_reading_starts_one_fallback_for_that_login_only() {
    // `b` is older than the 300 s default window.
    let s = two_logins([5, 301]);
    let fake = failing_server();
    let env = env_for(&fake, ProbeMode::Ordered);
    let mut calls = Vec::new();
    for name in ["a", "b"] {
        let fb = Fallback::new();
        let p = read(&env, &plan_for(&s.root, name), &s.root, now(), |m| {
            fb.run(m)
        });
        for m in fb.calls() {
            calls.push((name, m));
        }
        let want = if name == "a" {
            Source::Passive
        } else {
            Source::Fallback
        };
        assert_eq!(p.source, Some(want), "{name}");
    }
    assert_eq!(calls, [("b", ProbeMode::Auto)]);
}

// ---------------------------------------------------------------------------
// Issue #2271: direct-read results that are final
//
// Contract:
// - In `Ordered` and `Direct`, a direct read that fails with `no oauth
//   token` or `no refresh token`, or (on Linux only) `no credentials file`
//   in a creds_dir that is a real directory (not a symlink, not a regular
//   file, and not the `.no-home` stand-in for a missing `$HOME`), gives
//   `Probe::not_signed_in(why)`, and the fallback closure is never called:
//   - `report.state` `NotLoggedIn`, no windows;
//   - `error` `Some(ErrorReason::NotSignedIn(why))`, shown as
//     `not signed in (<why>)`;
//   - `source` `Direct`, `direct_failure` `Some(why)`, spend `Unknown`, no
//     shape, no raw.
// - Every other reason behaves as before: a fallback in `Ordered`, an
//   error row in `Direct`. `credentials refused` is never final.
// - An expired token that has a refresh token, with refresh not allowed,
//   is `token expired` and falls back (D3).
// - The verdict holds even when the endpoints are unusable (D4).
// - `Auto` and `Text` do no direct read and are unchanged.
// ---------------------------------------------------------------------------

use super::usage_direct::reason;

fn assert_not_signed_in(p: &Probe, why: &'static str) {
    assert_eq!(p.report.state, UsageState::NotLoggedIn, "{p:?}");
    assert!(p.report.session.is_none() && p.report.week_all.is_none());
    assert_eq!(p.error, Some(ErrorReason::NotSignedIn(why)), "{p:?}");
    assert_eq!(
        p.error.map(|e| e.to_string()).as_deref(),
        Some(format!("not signed in ({why})").as_str())
    );
    assert_eq!(p.source, Some(Source::Direct));
    assert_eq!(p.direct_failure, Some(why));
    assert_eq!(p.spend, Spend::Unknown);
    assert!(p.extra_shape.is_none());
    assert!(p.raw.is_none());
}

/// Rewrites the profile's login file with `doc`, kept private.
fn write_creds(s: &Setup, doc: &str) {
    let creds = s.dir().join(".credentials.json");
    fs::write(&creds, doc).unwrap();
    fs::set_permissions(&creds, fs::Permissions::from_mode(0o600)).unwrap();
}

fn expired_without_refresh_token() -> String {
    json!({"claudeAiOauth": {
        "accessToken": ACCESS,
        "expiresAt": chrono::Utc::now().timestamp_millis() - 1000
    }})
    .to_string()
}

/// A server that answers everything, so a fallback can only come from the
/// reason the direct read gave.
fn token_server() -> FakeHttp {
    FakeHttp::start(|s, _| {
        if s.path == "/v1/oauth/token" {
            Reply::json(
                200,
                &json!({"access_token": "FAKE-NEW-2271", "expires_in": 3600}),
            )
        } else {
            Reply::json(401, &json!({}))
        }
    })
}

#[test]
fn not_signed_in_has_the_documented_text_and_fields() {
    for why in [reason::NO_TOKEN, reason::NO_FILE, reason::NO_REFRESH_TOKEN] {
        let p = Probe::not_signed_in(why);
        assert_not_signed_in(&p, why);
    }
    assert_eq!(
        ErrorReason::NotSignedIn(reason::NO_TOKEN).to_string(),
        "not signed in (no oauth token)"
    );
}

/// Only a document that says there is no login is final: an empty object,
/// or a `claudeAiOauth` object with no access token (missing, `null` or
/// empty).
#[test]
fn a_file_with_no_token_is_final_in_ordered() {
    for doc in [
        "{}",
        r#"{"claudeAiOauth": {}}"#,
        r#"{"claudeAiOauth": {"accessToken": ""}}"#,
        r#"{"claudeAiOauth": {"accessToken": null}}"#,
        r#"{"claudeAiOauth": {"refreshToken": "sk-ant-oat01-TEST-2271-CANARY"}}"#,
    ] {
        let s = setup();
        write_creds(&s, doc);
        let fake = ok_server();
        let fb = Fallback::new();
        let p = read(
            &env_for(&fake, ProbeMode::Ordered),
            &s.plan(),
            &s.root,
            now(),
            |m| fb.run(m),
        );
        assert!(fb.calls().is_empty(), "{doc}: {:?}", fb.calls());
        assert_not_signed_in(&p, reason::NO_TOKEN);
        assert!(fake.seen().is_empty(), "{doc}");
    }
}

/// A file this read cannot make sense of is not a verdict on the login: the
/// format is not a documented contract, and a file may be caught mid-write.
/// It is `unreadable credentials` and falls back to `claude`, as every
/// direct failure did before #2271. `{"other": true}` is a judgement call:
/// a non-empty document with no `claudeAiOauth` may be an API-key login
/// (`primaryApiKey`) or a renamed key, and only `claude` can tell.
#[test]
fn an_unreadable_file_falls_back_in_ordered() {
    for doc in [
        "not json at all",
        r#"{"claudeAiOauth": {"accessToken": "sk-ant-oat01-TEST-2271-CANA"#,
        "[]",
        "null",
        r#"{"other": true}"#,
        r#"{"primaryApiKey": "sk-not-oauth"}"#,
        r#"{"claudeAiOauth": null}"#,
        r#"{"claudeAiOauth": "token"}"#,
        r#"{"claudeAiOauth": {"accessToken": 17}}"#,
        r#"{"claudeAiOauth": {"accessToken": "with space"}}"#,
    ] {
        let s = setup();
        write_creds(&s, doc);
        let fake = ok_server();
        let fb = Fallback::new();
        let p = read(
            &env_for(&fake, ProbeMode::Ordered),
            &s.plan(),
            &s.root,
            now(),
            |m| fb.run(m),
        );
        assert_falls_back_once(&p, &fb, reason::UNREADABLE_CREDS);
        assert!(fake.seen().is_empty(), "{doc}");
    }
}

/// Under `direct` the same file is an error row, never `not signed in`.
#[test]
fn an_unreadable_file_is_a_direct_error_not_not_signed_in() {
    let s = setup();
    write_creds(&s, "not json at all");
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty(), "{:?}", fb.calls());
    assert_eq!(p.error, Some(ErrorReason::Direct(reason::UNREADABLE_CREDS)));
    assert_eq!(p.direct_failure, Some(reason::UNREADABLE_CREDS));
    assert_ne!(p.report.state, UsageState::NotLoggedIn);
}

/// An expired login whose refresh token is there but rejected (a space in
/// it, or not a string) is not known to have no refresh token: it falls
/// back instead of reading `no refresh token`.
#[test]
fn an_expired_login_with_a_rejected_refresh_token_falls_back() {
    for refresh in [json!("has a space"), json!(17), json!("x".repeat(5000))] {
        let _serial = crate::test_support::refresh_count_lock();
        let s = setup();
        write_creds(
            &s,
            &json!({"claudeAiOauth": {
                "accessToken": ACCESS,
                "refreshToken": refresh,
                "expiresAt": chrono::Utc::now().timestamp_millis() - 1000
            }})
            .to_string(),
        );
        let fake = token_server();
        let fb = Fallback::new();
        let p = read(
            &env_for(&fake, ProbeMode::Ordered),
            &s.plan(),
            &s.root,
            now(),
            |m| fb.run(m),
        );
        assert_falls_back_once(&p, &fb, reason::UNREADABLE_CREDS);
        assert!(fake.seen().is_empty(), "{:?}", fake.seen());
    }
}

/// An empty or `null` refresh token is absent: an expired login with one is
/// final `no refresh token`.
#[test]
fn an_expired_login_with_an_empty_refresh_token_is_final() {
    for refresh in [json!(""), json!(null)] {
        let _serial = crate::test_support::refresh_count_lock();
        let s = setup();
        write_creds(
            &s,
            &json!({"claudeAiOauth": {
                "accessToken": ACCESS,
                "refreshToken": refresh,
                "expiresAt": chrono::Utc::now().timestamp_millis() - 1000
            }})
            .to_string(),
        );
        let fake = token_server();
        let fb = Fallback::new();
        let p = read(
            &env_for(&fake, ProbeMode::Ordered),
            &s.plan(),
            &s.root,
            now(),
            |m| fb.run(m),
        );
        assert!(fb.calls().is_empty(), "{:?}", fb.calls());
        assert_not_signed_in(&p, reason::NO_REFRESH_TOKEN);
    }
}

#[test]
fn an_expired_login_with_no_refresh_token_is_final_and_never_posts() {
    let _serial = crate::test_support::refresh_count_lock();
    let s = setup();
    write_creds(&s, &expired_without_refresh_token());
    let before = fs::read(s.dir().join(".credentials.json")).unwrap();
    let fake = token_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty(), "{:?}", fb.calls());
    assert_not_signed_in(&p, reason::NO_REFRESH_TOKEN);
    assert!(fake.seen().is_empty(), "{:?}", fake.seen());
    assert_eq!(fs::read(s.dir().join(".credentials.json")).unwrap(), before);
}

#[test]
fn direct_mode_gives_not_signed_in_not_a_direct_error() {
    let mut cases = vec![
        (Some("{}".to_string()), reason::NO_TOKEN),
        (
            Some(expired_without_refresh_token()),
            reason::NO_REFRESH_TOKEN,
        ),
    ];
    if cfg!(target_os = "linux") {
        cases.push((None, reason::NO_FILE));
    }
    for (doc, why) in cases {
        let _serial = crate::test_support::refresh_count_lock();
        let s = setup();
        match &doc {
            None => fs::remove_file(s.dir().join(".credentials.json")).unwrap(),
            Some(d) => write_creds(&s, d),
        }
        let fake = token_server();
        let fb = Fallback::new();
        let p = read(
            &env_for(&fake, ProbeMode::Direct),
            &s.plan(),
            &s.root,
            now(),
            |m| fb.run(m),
        );
        assert!(fb.calls().is_empty());
        assert_not_signed_in(&p, why);
        assert_ne!(p.report.state, UsageState::Error, "{why}");
    }
}

/// D2: with no `$HOME`, the live login's directory is a `.no-home` path
/// that does not exist. That is an environment problem, not a sign that
/// the user is logged out, so it still falls back.
#[test]
fn no_file_in_a_directory_that_does_not_exist_still_falls_back() {
    let s = setup();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.creds_dir = s.dir().join(".no-home");
    plan.passive = Some(ProfileName::parse("spare").unwrap());
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Auto]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert_eq!(p.direct_failure, Some(reason::NO_FILE));
    assert!(p.error.is_none());
}

/// In `Direct` the same missing directory is the old error row, not
/// `not signed in`.
#[test]
fn direct_mode_keeps_the_error_for_a_directory_that_does_not_exist() {
    let s = setup();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.creds_dir = s.dir().join(".no-home");
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_eq!(p.error, Some(ErrorReason::Direct(reason::NO_FILE)));
    assert_eq!(p.report.state, UsageState::Error);
}

/// A symlinked directory is not followed to decide the row: it may point
/// at a login `claude` can use.
#[test]
fn no_file_in_a_symlinked_directory_still_falls_back() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    let link = s.root.join("linked");
    std::os::unix::fs::symlink(s.dir(), &link).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.creds_dir = link;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Auto]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert!(p.error.is_none(), "not a not-signed-in row: {:?}", p.error);
}

/// A `.no-home` path that exists as a directory is a real directory: the
/// rule looks only at what is on disk.
#[cfg(target_os = "linux")]
#[test]
fn a_real_no_home_directory_with_no_file_is_final() {
    let s = setup();
    let dir = s.dir().join(".no-home");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.creds_dir = dir;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_not_signed_in(&p, reason::NO_FILE);
}

/// A token-less row never looks up `claude`.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_not_signed_in_row_never_resolves_the_binary() {
    let n = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&n);
    let lazy = LazyBinary::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Some(fake_binary())
    });
    let s = setup();
    write_creds(&s, "{}");
    let fake = ok_server();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fallback_with(&lazy, |_bin| Fallback::new().run(m)),
    );
    assert_not_signed_in(&p, reason::NO_TOKEN);
    assert_eq!(n.load(Ordering::SeqCst), 0);
}

/// A fresh passive reading still wins: the direct read is never reached.
#[test]
fn a_fresh_passive_reading_comes_before_the_login_check() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    s.seed_passive();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(p.source, Some(Source::Passive));
    assert!(p.error.is_none());
}

/// `auto` and `text` do no direct read: a token-less profile still goes to
/// `claude`.
#[test]
fn auto_and_text_still_start_claude_for_a_token_less_profile() {
    for mode in [ProbeMode::Auto, ProbeMode::Text] {
        let s = setup();
        fs::remove_file(s.dir().join(".credentials.json")).unwrap();
        let fake = ok_server();
        let fb = Fallback::new();
        let p = read(&env_for(&fake, mode), &s.plan(), &s.root, now(), |m| {
            fb.run(m)
        });
        assert_eq!(fb.calls(), [mode]);
        assert_eq!(p.source, Some(Source::Fallback));
    }
}

/// An overflow profile is read with `/usage` only, whatever its login.
#[test]
fn an_overflow_profile_with_no_login_file_still_gets_the_text_probe() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.overflow = true;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Text]);
    assert_eq!(p.source, Some(Source::Fallback));
}

// Regression: every other reason still falls back exactly once.

fn assert_falls_back_once(p: &Probe, fb: &Fallback, why: &'static str) {
    assert_eq!(fb.calls(), [ProbeMode::Auto], "{why}");
    assert_eq!(p.source, Some(Source::Fallback), "{why}");
    assert_eq!(p.direct_failure, Some(why));
    assert!(p.error.is_none(), "{why}: {:?}", p.error);
}

#[test]
fn a_5xx_still_falls_back() {
    let s = setup();
    let fake = failing_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::HTTP_5XX);
}

/// A deadline with nothing left gives `timeout` before any request starts,
/// so nothing here waits.
#[test]
fn a_timeout_still_falls_back() {
    let s = setup();
    let fake = ok_server();
    let mut env = env_for(&fake, ProbeMode::Ordered);
    env.timeout = Duration::ZERO;
    let fb = Fallback::new();
    let p = read(&env, &s.plan(), &s.root, now(), |m| fb.run(m));
    assert_falls_back_once(&p, &fb, reason::TIMEOUT);
}

#[test]
fn a_swap_journal_refresh_busy_still_falls_back() {
    let s = setup();
    write_creds(
        &s,
        &json!({"claudeAiOauth": {
            "accessToken": ACCESS,
            "refreshToken": "FAKE-REFRESH-2271",
            "expiresAt": chrono::Utc::now().timestamp_millis() - 1000
        }})
        .to_string(),
    );
    let state = s.root.join(".auto-switch.d");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(state.join("journal.json"), "{}").unwrap();
    let fake = token_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::REFRESH_BUSY);
    assert_eq!(fake.count("POST", "/v1/oauth/token"), 0);
}

#[test]
fn a_directory_others_can_write_refresh_refused_still_falls_back() {
    let s = setup();
    write_creds(
        &s,
        &json!({"claudeAiOauth": {
            "accessToken": ACCESS,
            "refreshToken": "FAKE-REFRESH-2271",
            "expiresAt": chrono::Utc::now().timestamp_millis() - 1000
        }})
        .to_string(),
    );
    fs::set_permissions(s.dir(), fs::Permissions::from_mode(0o777)).unwrap();
    let fake = token_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    fs::set_permissions(s.dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert_falls_back_once(&p, &fb, reason::REFRESH_REFUSED);
}

/// D2: with refresh not allowed (the live `~/.claude` row), an expired
/// login with no refresh token is still final: `claude` cannot renew it
/// either.
#[test]
fn an_expired_login_with_no_refresh_token_is_final_without_permission_to_refresh() {
    let s = setup();
    write_creds(&s, &expired_without_refresh_token());
    let fake = token_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.allow_refresh = false;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty(), "{:?}", fb.calls());
    assert_not_signed_in(&p, reason::NO_REFRESH_TOKEN);
    assert!(fake.seen().is_empty());
}

/// D3: an expired token that has a refresh token, with refresh not
/// allowed, is `token expired` and still falls back: `claude` can renew it,
/// so the row is not "not signed in".
#[test]
fn token_expired_without_permission_to_refresh_still_falls_back() {
    let s = setup();
    write_creds(
        &s,
        &json!({"claudeAiOauth": {
            "accessToken": ACCESS,
            "refreshToken": "FAKE-REFRESH-2271",
            "expiresAt": chrono::Utc::now().timestamp_millis() - 1000
        }})
        .to_string(),
    );
    let fake = token_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.allow_refresh = false;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::EXPIRED);
    assert!(fake.seen().is_empty());
}

#[test]
fn a_refused_login_file_still_falls_back() {
    let s = setup();
    let creds = s.dir().join(".credentials.json");
    // Group-writable: refused before it is read, so a login may be there.
    fs::set_permissions(&creds, fs::Permissions::from_mode(0o664)).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::REFUSED);
}

/// S1: a symlinked login file is `credentials refused` and falls back: it
/// may point at a login `claude` can use, so it is never "not signed in".
#[test]
fn a_symlinked_login_file_still_falls_back() {
    let s = setup();
    let creds = s.dir().join(".credentials.json");
    let real = s.root.join("elsewhere.json");
    fs::rename(&creds, &real).unwrap();
    std::os::unix::fs::symlink(&real, &creds).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::REFUSED);
}

/// A refused file that holds no token is still refused, never final: the
/// verdict is not taken from a file that was not read.
#[test]
fn a_refused_file_with_no_token_still_falls_back() {
    let s = setup();
    write_creds(&s, "{}");
    let creds = s.dir().join(".credentials.json");
    fs::set_permissions(&creds, fs::Permissions::from_mode(0o620)).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::REFUSED);
}

/// S6: a creds_dir that is a regular file is not a real directory, so a
/// missing login inside it is not final.
#[test]
fn no_file_under_a_path_that_is_a_regular_file_still_falls_back() {
    let s = setup();
    let not_a_dir = s.root.join("plain-file");
    fs::write(&not_a_dir, "").unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let mut plan = s.plan();
    plan.creds_dir = not_a_dir;
    let p = read(
        &env_for(&fake, ProbeMode::Ordered),
        &plan,
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_eq!(fb.calls(), [ProbeMode::Auto]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert!(p.error.is_none(), "{:?}", p.error);
}

/// `Endpoints` the test guard refuses: `usable()` is false.
fn env_unusable(mode: ProbeMode) -> SourceEnv {
    let (endpoints, _) = Endpoints::resolve_with(|k| {
        (k == "AMPLIHACK_TEST_HOME_GUARD").then(|| std::ffi::OsString::from("1"))
    });
    assert!(!endpoints.usable());
    SourceEnv {
        mode,
        passive_max_age: 300,
        endpoints,
        timeout: Duration::from_secs(5),
    }
}

/// D4: a token-less profile is never sent to `claude` just because the
/// endpoints are refused; the login is judged first.
#[test]
fn unusable_endpoints_do_not_hide_a_token_less_profile() {
    for (doc, why) in [
        ("{}".to_string(), reason::NO_TOKEN),
        (expired_without_refresh_token(), reason::NO_REFRESH_TOKEN),
    ] {
        for mode in [ProbeMode::Ordered, ProbeMode::Direct] {
            let s = setup();
            write_creds(&s, &doc);
            let fb = Fallback::new();
            let p = read(&env_unusable(mode), &s.plan(), &s.root, now(), |m| {
                fb.run(m)
            });
            assert!(fb.calls().is_empty(), "{why} {mode:?}: {:?}", fb.calls());
            assert_not_signed_in(&p, why);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn unusable_endpoints_with_no_login_file_is_not_signed_in_on_linux() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    let fb = Fallback::new();
    let p = read(
        &env_unusable(ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_not_signed_in(&p, reason::NO_FILE);
}

/// With a usable token and refused endpoints the row still falls back
/// with `transport`, as before.
#[test]
fn unusable_endpoints_with_a_token_still_fall_back_with_transport() {
    let s = setup();
    let fb = Fallback::new();
    let p = read(
        &env_unusable(ProbeMode::Ordered),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert_falls_back_once(&p, &fb, reason::TRANSPORT);
}

/// D1 off Linux: in `Direct` a missing file keeps the old error row.
#[cfg(not(target_os = "linux"))]
#[test]
fn direct_mode_keeps_the_error_for_a_missing_file_off_linux() {
    let s = setup();
    fs::remove_file(s.dir().join(".credentials.json")).unwrap();
    let fake = ok_server();
    let fb = Fallback::new();
    let p = read(
        &env_for(&fake, ProbeMode::Direct),
        &s.plan(),
        &s.root,
        now(),
        |m| fb.run(m),
    );
    assert!(fb.calls().is_empty());
    assert_eq!(p.error, Some(ErrorReason::Direct(reason::NO_FILE)));
}

/// No not-signed-in row ever holds the token it found or did not find.
#[test]
fn a_not_signed_in_row_never_holds_a_token() {
    for doc in [
        r#"{"claudeAiOauth": {"refreshToken": "sk-ant-oat01-TEST-2271-CANARY"}}"#.to_string(),
        expired_without_refresh_token(),
    ] {
        let s = setup();
        write_creds(&s, &doc);
        let fake = ok_server();
        let p = read(
            &env_for(&fake, ProbeMode::Ordered),
            &s.plan(),
            &s.root,
            now(),
            |m| Fallback::new().run(m),
        );
        let shown = format!(
            "{p:?} {}",
            p.error.map(|e| e.to_string()).unwrap_or_default()
        );
        assert!(
            !shown.contains("CANARY") && !shown.contains(ACCESS),
            "{shown}"
        );
    }
}
