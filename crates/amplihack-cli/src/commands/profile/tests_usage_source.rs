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
use super::usage_source::{LazyBinary, Plan, Source, SourceEnv, fallback_with, read};
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

/// The `claude` probe stand-in: records the mode and gives 33%/44%.
struct Fallback {
    calls: RefCell<Vec<ProbeMode>>,
}

impl Fallback {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
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
                state: UsageState::Ok,
                session: Some(w("session", 33.0)),
                week_all: Some(w("all models", 44.0)),
                week_models: Vec::new(),
                session_limit: true,
            },
            error: None,
            raw: None,
            source: Some(Source::Fallback),
            direct_failure: None,
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

#[test]
fn ordered_falls_back_to_text_when_credits_are_on() {
    let s = setup();
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body(10.0, 20.0, true)));
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
    assert_eq!(p.direct_failure, Some("no credentials file"));
    assert!(fake.seen().is_empty());
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
