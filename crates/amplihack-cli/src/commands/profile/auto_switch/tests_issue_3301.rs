//! Model observations survive unrelated updates without changing their age.

use super::state::{HostEntry, Ledger, State};
use super::usage::{self, ModelWeek, Reading, ReadingSource, Window};
use super::{Observation, ObservedWindow, RecordOutcome, record_usage_at};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::usage_direct::Endpoints;
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source::{self, Plan, Source, SourceEnv};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const NOW: i64 = 1_793_534_400;
const RESET: i64 = NOW + 3600;

fn observation(models: &[(&str, f64, Option<i64>)]) -> Observation {
    Observation {
        profile: "spare".into(),
        account: "spare".into(),
        session: Some(ObservedWindow {
            used_percentage: 10.0,
            resets_at: Some(NOW + 7200),
        }),
        week: Some(ObservedWindow {
            used_percentage: 20.0,
            resets_at: Some(NOW + 86400),
        }),
        week_models: models
            .iter()
            .map(|(label, pct, reset)| {
                (
                    label.to_string(),
                    ObservedWindow {
                        used_percentage: *pct,
                        resets_at: *reset,
                    },
                )
            })
            .collect(),
    }
}

fn state() -> (tempfile::TempDir, State) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(tmp.path().join("spare")).unwrap();
    let state = State::new(tmp.path()).unwrap();
    state.ensure().unwrap();
    (tmp, state)
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
        panic!("stored reading expected")
    })
}

#[test]
fn issue_3301_recorded_models_survive_passive_reads_and_expire_at_reset() {
    let (_tmp, state) = state();
    record_usage_at(
        &state.root,
        &[observation(&[("Fable", 100.0, Some(RESET))])],
        NOW,
    )
    .unwrap();
    let first = passive(&state, NOW + 1);
    assert_eq!(first.source, Some(Source::Passive));
    assert_eq!(first.report.state, UsageState::WeeklyExhausted);
    assert_eq!(first.report.week_models[0].used_percentage, 100.0);
    assert_eq!(first.report.week_models[0].observed_at, Some(NOW));
    let bytes = fs::read(state.dir.join("usage.json")).unwrap();
    let second = passive(&state, NOW + 2);
    assert_eq!(second.report.week_models, first.report.week_models);
    assert_eq!(fs::read(state.dir.join("usage.json")).unwrap(), bytes);

    // An unrelated direct observation updates the session and all-models week.
    record_usage_at(&state.root, &[observation(&[])], NOW + 10).unwrap();
    let retained = passive(&state, RESET - 1);
    assert_eq!(retained.report.week_models, first.report.week_models);
    assert_eq!(retained.report.observed_at, Some(NOW + 10));
    assert!(passive(&state, RESET).report.week_models.is_empty());
    assert_eq!(passive(&state, RESET).report.state, UsageState::Ok);
}

#[test]
fn issue_3301_models_update_individually_and_need_a_future_reset_for_retention() {
    let (_tmp, state) = state();
    record_usage_at(
        &state.root,
        &[observation(&[
            ("Fable", 100.0, Some(RESET)),
            ("Sonnet", 30.0, Some(RESET)),
        ])],
        NOW,
    )
    .unwrap();
    record_usage_at(
        &state.root,
        &[observation(&[("Sonnet", 15.0, Some(RESET))])],
        NOW + 10,
    )
    .unwrap();
    let p = passive(&state, NOW + 11);
    assert_eq!(p.report.week_models.len(), 2);
    assert_eq!(p.report.week_models[0].label, "Fable");
    assert_eq!(p.report.week_models[0].observed_at, Some(NOW));
    assert_eq!(p.report.week_models[1].used_percentage, 15.0);
    assert_eq!(p.report.week_models[1].observed_at, Some(NOW + 10));
    record_usage_at(
        &state.root,
        &[observation(&[
            ("Fable", 0.0, None),
            ("Sonnet", 0.0, Some(NOW + 20)),
        ])],
        NOW + 20,
    )
    .unwrap();
    assert!(passive(&state, NOW + 21).report.week_models.is_empty());
}

#[test]
fn issue_3301_account_change_does_not_record_model_figures() {
    let (_tmp, state) = state();
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
    let result = record_usage_at(
        &state.root,
        &[observation(&[("Fable", 100.0, Some(RESET))])],
        NOW,
    )
    .unwrap();
    assert_eq!(
        result,
        RecordOutcome::Recorded {
            written: vec![],
            moved: vec!["spare".into()]
        }
    );
    assert!(state.try_load_usage().unwrap().accounts.is_empty());
}

#[test]
fn issue_3301_statusline_merge_preserves_each_models_observation_time() {
    let old = Reading {
        five_hour: None,
        seven_day: Some(Window {
            used_percentage: 20.0,
            resets_at: NOW + 86400,
        }),
        observed_at: NOW,
        source: ReadingSource::Statusline,
        week_models: vec![ModelWeek {
            label: "Fable".into(),
            used_percentage: 100.0,
            resets_at: Some(RESET),
            observed_at: NOW,
        }],
    };
    let raw = serde_json::json!({"rate_limits": {
        "seven_day": {"used_percentage": 21.0, "resets_at": NOW + 86400},
    }});
    let limits =
        usage::parse_statusline_input(&serde_json::to_vec(&raw).unwrap(), NOW + 10).unwrap();
    let merged = usage::merge(Some(&old), usage::reading_from(&limits, NOW + 10));
    assert_eq!(merged.week_models, old.week_models);
    assert_eq!(merged.observed_at, NOW + 10);
    let expired = usage::merge(Some(&merged), usage::reading_from(&limits, RESET));
    assert!(expired.week_models.is_empty());
}
