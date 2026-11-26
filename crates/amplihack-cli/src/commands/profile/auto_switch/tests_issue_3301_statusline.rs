//! Status-line observations pass through attribution, storage and passive reads.

use super::super::state::{HostEntry, Ledger, State};
use super::record_at;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::usage_direct::Endpoints;
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source::{self, Plan, Source, SourceEnv};
use crate::test_support::{EnvGuard, env_lock};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const NOW: i64 = 1_793_534_400;
const RESET: i64 = NOW + 3600;

fn state() -> (tempfile::TempDir, State) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(tmp.path().join("spare")).unwrap();
    let toggle = tmp.path().join(".auto-switch");
    fs::write(&toggle, "enabled=on\nthreshold=95\nexclude=\n").unwrap();
    fs::set_permissions(toggle, fs::Permissions::from_mode(0o600)).unwrap();
    let state = State::new(tmp.path()).unwrap();
    state.ensure().unwrap();
    (tmp, state)
}

fn input(models: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"rate_limits": {
        "five_hour": {"used_percentage": 10, "resets_at": NOW + 7200},
        "seven_day": {"used_percentage": 20, "resets_at": NOW + 86400},
        "model_scoped": models,
    }}))
    .unwrap()
}

fn passive(state: &State, now: i64) -> crate::commands::profile::usage_cmd::Probe {
    let plan = Plan {
        creds_dir: state.root.join("spare"),
        passive: Some(ProfileName::parse("spare").unwrap()),
        allow_refresh: false,
        overflow: false,
    };
    let env = SourceEnv {
        mode: ProbeMode::Ordered,
        passive_max_age: 86400,
        endpoints: Endpoints::resolve(None, None).0,
        timeout: Duration::from_secs(5),
    };
    usage_source::read(&env, &plan, &state.root, now, |_| {
        panic!("a passive reading must not start a probe")
    })
}

#[test]
fn issue_3301_statusline_models_persist_with_individual_ages_and_expire() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(["AMPLIHACK_CLAUDE_AUTO_SWITCH"]);
    let (_tmp, state) = state();
    let owner = ProfileName::parse("spare").unwrap();
    let models = json!([
        {"display_name": "Fable", "used_percentage": 100, "resets_at": RESET},
        {"display_name": "Sonnet", "used_percentage": 40,
         "resets_at": chrono::DateTime::from_timestamp(RESET, 0).unwrap().to_rfc3339()},
    ]);
    record_at(&state, &owner, &input(models), false, NOW).unwrap();
    let first = passive(&state, NOW + 1);
    assert_eq!(first.source, Some(Source::Passive));
    assert_eq!(first.report.state, UsageState::WeeklyExhausted);
    assert_eq!(first.report.week_models.len(), 2);
    assert_eq!(first.report.week_models[0].label, "Fable");
    assert_eq!(first.report.week_models[0].used_percentage, 100.0);
    assert_eq!(first.report.week_models[0].observed_at, Some(NOW));
    assert_eq!(first.report.week_models[0].resets_at, Some(RESET));

    let update = json!([
        {"display_name": "Sonnet", "used_percentage": 30, "resets_at": RESET},
        {"display_name": "", "used_percentage": 100, "resets_at": RESET},
        {"display_name": "bad\u{001b}", "used_percentage": 100, "resets_at": RESET},
        {"display_name": "unmeasured", "resets_at": RESET},
        {"display_name": "unknown-reset", "used_percentage": 100},
    ]);
    record_at(&state, &owner, &input(update), false, NOW + 10).unwrap();
    let later = passive(&state, NOW + 11);
    assert_eq!(later.report.week_models.len(), 2);
    assert_eq!(later.report.week_models[0], first.report.week_models[0]);
    assert_eq!(later.report.week_models[1].used_percentage, 30.0);
    assert_eq!(later.report.week_models[1].observed_at, Some(NOW + 10));

    // Missing model data in an older status-line payload is not a measurement.
    let mut old_payload: Value = serde_json::from_slice(&input(Value::Null)).unwrap();
    old_payload["rate_limits"]
        .as_object_mut()
        .unwrap()
        .remove("model_scoped");
    record_at(
        &state,
        &owner,
        &serde_json::to_vec(&old_payload).unwrap(),
        false,
        NOW + 20,
    )
    .unwrap();
    assert_eq!(
        passive(&state, RESET - 1).report.week_models,
        later.report.week_models
    );
    let bytes = fs::read(state.dir.join("usage.json")).unwrap();
    let expired = passive(&state, RESET);
    assert!(expired.report.week_models.is_empty());
    assert_eq!(expired.report.state, UsageState::Ok);
    assert_eq!(fs::read(state.dir.join("usage.json")).unwrap(), bytes);
}

#[test]
fn issue_3301_statusline_models_use_existing_account_attribution() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(["AMPLIHACK_CLAUDE_AUTO_SWITCH"]);
    let (_tmp, state) = state();
    let owner = ProfileName::parse("spare").unwrap();
    let mut ledger = Ledger {
        version: 1,
        ..Ledger::default()
    };
    ledger.hosts.insert(
        "spare".into(),
        HostEntry {
            live: "other".into(),
            swapped_at: NOW - 1000,
            pre_swap_fingerprint: None,
        },
    );
    state.save_ledger(&ledger).unwrap();
    // A model-only payload is still a usable reading.
    let raw = serde_json::to_vec(&json!({"rate_limits": {"model_scoped": [
        {"display_name": "Fable", "used_percentage": 100, "resets_at": RESET}
    ]}}))
    .unwrap();
    record_at(&state, &owner, &raw, false, NOW).unwrap();
    let file = state.try_load_usage().unwrap();
    assert!(!file.accounts.contains_key("spare"));
    assert_eq!(file.accounts["other"].week_models[0].used_percentage, 100.0);
    assert_eq!(
        passive(&state, NOW + 1).report.state,
        UsageState::WeeklyExhausted
    );

    ledger.hosts.get_mut("spare").unwrap().swapped_at = NOW + 10;
    state.save_ledger(&ledger).unwrap();
    let before = fs::read(state.dir.join("usage.json")).unwrap();
    record_at(&state, &owner, &raw, false, NOW + 11).unwrap();
    assert_eq!(fs::read(state.dir.join("usage.json")).unwrap(), before);
}
