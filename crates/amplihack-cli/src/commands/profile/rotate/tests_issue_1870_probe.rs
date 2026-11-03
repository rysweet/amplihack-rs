//! Issue #1870 review (round 3): the freshness gate's re-read follows the
//! user's probe method, `AMPLIHACK_PROFILE_USAGE_PROBE`.
//!
//! Contract (`rotate::fresh`):
//! - `fresh::reread_env(mode, left) -> SourceEnv` keeps `mode` as given,
//!   sets `passive_max_age` to 0 and the deadline to `left`.
//! - Read through `usage_source::read`, that env gives:
//!
//!   | Mode | Re-read |
//!   |---|---|
//!   | `text`, `auto` | the `claude` fallback only, as named; no direct read |
//!   | unset | direct; on failure the fallback (`text` when credits are on) |
//!   | `direct` | direct only; a failure is an error row |
//!
//!   and never uses a stored (passive) reading, however fresh.
//! - `fresh::reread_as_configured` builds its env with `reread_env` and the
//!   mode `measure::mode_quiet()` resolves from the environment (no second
//!   warning: the cycle's measurement has already warned once).
//!
//! Direct reads go to a loopback fake; the fallback is a closure that
//! records its calls. No `claude` is started and no variable is set.

use super::fresh::reread_env;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::{ProbeMode, probe_mode};
use crate::commands::profile::tests_fake_http::{FakeHttp, Reply};
use crate::commands::profile::usage_cmd::Probe;
use crate::commands::profile::usage_direct::Endpoints;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};
use crate::commands::profile::usage_source::{Plan, Source, Spend, read};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn iso(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A profile root with `spare` logged in and a fresh stored reading of
/// 7%/9% for it, which a re-read must never use.
struct Root {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("profiles");
    let state = root.join(".auto-switch.d");
    fs::create_dir_all(root.join("spare")).unwrap();
    fs::create_dir_all(&state).unwrap();
    for d in [&root, &root.join("spare"), &state] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let root = root.canonicalize().unwrap();
    let private = |p: PathBuf, body: String| {
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    };
    private(
        root.join("spare/.credentials.json"),
        json!({"claudeAiOauth": {
            "accessToken": "FAKE-ACCESS-1870-probe",
            "refreshToken": "FAKE-REFRESH-1870-probe",
            "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000
        }})
        .to_string(),
    );
    let n = now();
    private(
        root.join(".auto-switch.d/usage.json"),
        json!({"version": 1, "accounts": {"spare": {
            "five_hour": {"used_percentage": 7.0, "resets_at": n + 3600},
            "seven_day": {"used_percentage": 9.0, "resets_at": n + 86_400},
            "observed_at": n - 5,
            "source": "statusline"
        }}})
        .to_string(),
    );
    Root { _tmp: tmp, root }
}

fn plan(r: &Root) -> Plan {
    Plan {
        creds_dir: r.root.join("spare"),
        passive: Some(ProfileName::parse("spare").unwrap()),
        allow_refresh: false,
        overflow: false,
    }
}

/// The mode a user's `AMPLIHACK_PROFILE_USAGE_PROBE` value resolves to.
fn user_mode(value: Option<&str>) -> ProbeMode {
    let (mode, warning) = probe_mode(value.map(OsStr::new));
    assert_eq!(warning, None, "{value:?}");
    mode
}

/// The re-read's env for `value`, with the endpoints pointed at `fake`.
fn reread_env_for(
    fake: &FakeHttp,
    value: Option<&str>,
) -> crate::commands::profile::usage_source::SourceEnv {
    let mut env = reread_env(user_mode(value), Duration::from_secs(5));
    let (endpoints, _) = Endpoints::resolve(
        Some(OsStr::new(&fake.base())),
        Some(OsStr::new(&fake.token_url())),
    );
    env.endpoints = endpoints;
    env
}

/// The `claude` probe stand-in: records each mode and reads 43%/100%.
struct Claude(RefCell<Vec<ProbeMode>>);

impl Claude {
    fn new() -> Self {
        Self(RefCell::new(Vec::new()))
    }

    fn run(&self, mode: ProbeMode) -> Probe {
        self.0.borrow_mut().push(mode);
        let w = |label: &str, p: f64| UsageWindow {
            label: label.into(),
            used_percentage: p,
            resets_text: None,
            resets_at: None,
        };
        Probe {
            report: UsageReport {
                state: UsageState::WeeklyExhausted,
                session: Some(w("session", 43.0)),
                week_all: Some(w("all models", 100.0)),
                week_models: Vec::new(),
                session_limit: true,
                observed_at: None,
            },
            error: None,
            raw: None,
            source: Some(Source::Fallback),
            direct_failure: None,
            spend: Spend::Ok,
            extra_shape: None,
        }
    }

    fn calls(&self) -> Vec<ProbeMode> {
        self.0.borrow().clone()
    }
}

fn week(p: &Probe) -> Option<f64> {
    p.report.week_all.as_ref().map(|w| w.used_percentage)
}

fn usage_body(session: f64, week: f64, credits: bool) -> Value {
    let n = now();
    json!({
        "five_hour": {"utilization": session, "resets_at": iso(n + 3600)},
        "seven_day": {"utilization": week, "resets_at": iso(n + 3 * 86_400)},
        "extra_usage": {"is_enabled": credits}
    })
}

#[test]
fn reread_env_keeps_the_users_mode_turns_passive_off_and_uses_the_time_left() {
    let left = Duration::from_millis(1234);
    for value in [None, Some("direct"), Some("auto"), Some("text")] {
        let mode = user_mode(value);
        let env = reread_env(mode, left);
        assert_eq!(env.mode, mode, "{value:?}: the user's mode, never forced");
        assert_eq!(env.passive_max_age, 0, "{value:?}: no stored reading");
        assert_eq!(env.timeout, left, "{value:?}");
    }
}

#[test]
fn under_text_and_auto_the_reread_runs_claude_as_named_and_never_reads_directly() {
    for (value, named) in [("text", ProbeMode::Text), ("auto", ProbeMode::Auto)] {
        let r = root();
        let fake = FakeHttp::start(|_, _| Reply::json(200, &usage_body(1.0, 1.0, false)));
        let claude = Claude::new();
        let p = read(
            &reread_env_for(&fake, Some(value)),
            &plan(&r),
            &r.root,
            now(),
            |m| claude.run(m),
        );
        assert_eq!(
            claude.calls(),
            [named],
            "{value}: one claude read, as named"
        );
        assert!(fake.seen().is_empty(), "{value}: no direct read was made");
        assert_eq!(p.source, Some(Source::Fallback), "{value}");
        assert_eq!(week(&p), Some(100.0), "{value}: claude's figures");
    }
}

#[test]
fn unset_with_credits_on_and_no_window_falls_back_to_text_rather_than_failing() {
    let r = root();
    let fake = FakeHttp::start(|_, _| {
        Reply::json(
            200,
            &json!({"five_hour": null, "extra_usage": {"is_enabled": true}}),
        )
    });
    let claude = Claude::new();
    let p = read(
        &reread_env_for(&fake, None),
        &plan(&r),
        &r.root,
        now(),
        |m| claude.run(m),
    );
    assert_eq!(claude.calls(), [ProbeMode::Text]);
    assert_eq!(p.source, Some(Source::Fallback));
    assert_eq!(p.direct_failure, Some("usage credits on"));
    assert!(p.error.is_none(), "a reading, so the target is not unknown");
}

#[test]
fn unset_reads_directly_and_skips_the_fresh_stored_reading() {
    let r = root();
    let fake = FakeHttp::start(|_, _| Reply::json(200, &usage_body(43.0, 100.0, false)));
    let claude = Claude::new();
    let p = read(
        &reread_env_for(&fake, None),
        &plan(&r),
        &r.root,
        now(),
        |m| claude.run(m),
    );
    assert_eq!(
        p.source,
        Some(Source::Direct),
        "not the 7%/9% stored reading"
    );
    assert_eq!(week(&p), Some(100.0));
    assert!(claude.calls().is_empty());
}

#[test]
fn under_direct_a_failed_reread_is_an_error_and_starts_no_claude() {
    let r = root();
    let fake = FakeHttp::start(|_, _| Reply::raw(500, ""));
    let claude = Claude::new();
    let p = read(
        &reread_env_for(&fake, Some("direct")),
        &plan(&r),
        &r.root,
        now(),
        |m| claude.run(m),
    );
    assert!(claude.calls().is_empty(), "direct means no claude");
    assert!(p.error.is_some(), "the target is unknown for the cycle");
}

#[test]
fn the_production_reread_resolves_the_mode_from_the_environment() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate");
    let text = fs::read_to_string(dir.join("fresh.rs")).unwrap();
    let body = text
        .split("pub fn reread_as_configured(")
        .nth(1)
        .expect("fresh::reread_as_configured exists");
    let body = &body[..body.find("\n}\n").unwrap()];
    assert!(
        body.contains("reread_env(measure::mode_quiet(), left)"),
        "the re-read uses AMPLIHACK_PROFILE_USAGE_PROBE: {body}"
    );
    assert!(
        !body.contains("ProbeMode::"),
        "no mode is forced on the re-read: {body}"
    );
}
