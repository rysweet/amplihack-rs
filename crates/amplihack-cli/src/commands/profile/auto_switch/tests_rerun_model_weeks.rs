//! A session-limit failure must not erase independent model-week evidence.

use super::record_limit;
use crate::commands::profile::auto_switch::kit::{State, now};
use crate::commands::profile::auto_switch::usage::{ModelWeek, ReadingSource};
use crate::commands::profile::auto_switch::{Observation, ObservedWindow, record_usage_at};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::usage_direct::Endpoints;
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source::{self, Plan, Source, SourceEnv};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn issue_3301_limit_recording_preserves_model_weeks_through_passive_reconstruction() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(tmp.path().join("spare")).unwrap();
    let state = State::new(tmp.path()).unwrap();
    state.ensure().unwrap();
    let observed = now() - 60;
    let model_reset = observed + 7200;
    record_usage_at(
        tmp.path(),
        &[Observation {
            profile: "spare".into(),
            account: "spare".into(),
            session: Some(ObservedWindow {
                used_percentage: 10.0,
                resets_at: Some(observed + 3600),
            }),
            week: Some(ObservedWindow {
                used_percentage: 20.0,
                resets_at: Some(observed + 86400),
            }),
            week_models: vec![(
                "Fable".into(),
                ObservedWindow {
                    used_percentage: 100.0,
                    resets_at: Some(model_reset),
                },
            )],
        }],
        observed,
    )
    .unwrap();
    let mut file = state.try_load_usage().unwrap();
    let original = file.accounts["spare"].week_models[0].clone();
    // Legacy/malformed observations with no future reset must not be retained.
    for (label, reset) in [("expired", Some(observed)), ("undated", None)] {
        file.accounts
            .get_mut("spare")
            .unwrap()
            .week_models
            .push(ModelWeek {
                label: label.into(),
                used_percentage: 100.0,
                resets_at: reset,
                observed_at: observed,
            });
    }
    state.save_usage(&file).unwrap();

    let session_reset = observed + 1800;
    record_limit(&state, "spare", session_reset);
    let saved = state.try_load_usage().unwrap();
    let reading = &saved.accounts["spare"];
    assert_eq!(reading.source, ReadingSource::UsageLimitFailure);
    assert_eq!(reading.five_hour.unwrap().used_percentage, 100.0);
    assert_eq!(reading.five_hour.unwrap().resets_at, session_reset);
    assert_eq!(reading.seven_day, file.accounts["spare"].seven_day);
    assert_eq!(reading.week_models, [original]);
    assert!(reading.observed_at > observed);

    let env = SourceEnv {
        mode: ProbeMode::Ordered,
        passive_max_age: 86400,
        endpoints: Endpoints::resolve(None, None).0,
        timeout: Duration::from_secs(5),
    };
    let plan = Plan {
        creds_dir: tmp.path().join("spare"),
        passive: Some(ProfileName::parse("spare").unwrap()),
        allow_refresh: false,
        overflow: false,
    };
    let passive = |at| {
        usage_source::read(&env, &plan, tmp.path(), at, |_| {
            panic!("stored reading must reconstruct without a probe")
        })
    };
    let retained = passive(now());
    assert_eq!(retained.source, Some(Source::Passive));
    assert_eq!(retained.report.state, UsageState::SessionExhausted);
    assert_eq!(retained.report.week_models.len(), 1);
    assert_eq!(retained.report.week_models[0].label, "Fable");
    assert_eq!(retained.report.week_models[0].used_percentage, 100.0);
    assert_eq!(retained.report.week_models[0].observed_at, Some(observed));
    // A later session-only observation must keep the model evidence too.
    record_usage_at(
        tmp.path(),
        &[Observation {
            profile: "spare".into(),
            account: "spare".into(),
            session: Some(ObservedWindow {
                used_percentage: 0.0,
                resets_at: Some(model_reset),
            }),
            week: None,
            week_models: vec![],
        }],
        now() + 1,
    )
    .unwrap();
    assert_eq!(passive(now() + 2).report.state, UsageState::WeeklyExhausted);
    assert_eq!(passive(model_reset).report.state, UsageState::Ok);
    assert!(passive(model_reset).report.week_models.is_empty());
}
