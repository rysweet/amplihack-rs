//! Direct observations use the rotator's reader and account-attributed recorder.

use super::cycle::Ctx;
use super::decide::Reading;
use super::measure;
use super::{TuneFlags, settings};
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::tests_fake_http::{FakeHttp, Reply};
use crate::commands::profile::usage_cmd::{ENV_CREDENTIAL_VARS, Probe};
use crate::commands::profile::usage_direct::Endpoints;
use crate::commands::profile::usage_parse::{UsageReport, parse_usage};
use crate::commands::profile::usage_request::{REQUEST_ID, parse_answer};
use crate::commands::profile::usage_source::{Source, SourceEnv};
use crate::test_support::{EnvGuard, env_lock};
use serde_json::json;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn setup() -> (tempfile::TempDir, Ctx, State) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    let home = tmp.path().join("home");
    for p in [&root, &root.join("spare"), &home] {
        fs::create_dir_all(p).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    let settings = settings::resolve(&TuneFlags::default(), None, &|_| None).unwrap();
    let ctx = Ctx::new(ProfileStore::new(root), home, settings);
    (tmp, ctx, state)
}

pub(super) fn env(mode: ProbeMode) -> SourceEnv {
    SourceEnv {
        mode,
        passive_max_age: 300,
        endpoints: Endpoints::resolve(None, None).0,
        timeout: Duration::from_secs(5),
    }
}

pub(super) fn report(mode: ProbeMode, now: i64, models: bool) -> UsageReport {
    let reset = chrono::DateTime::from_timestamp(now + 86400, 0).unwrap();
    if mode == ProbeMode::Text {
        let at = reset.format("%b %-d, %-I:%M%P (Etc/UTC)");
        let model = if models {
            format!("Current week (Fable): 100% used · resets {at}\n")
        } else {
            String::new()
        };
        return parse_usage(
            &format!(
                "Current session: 10% used · resets {at}\n\
                 Current week (all models): 20% used · resets {at}\n{model}"
            ),
            "",
            now,
        );
    }
    let models = if models {
        json!([{"display_name": "Fable", "utilization": 100, "resets_at": reset.to_rfc3339()}])
    } else {
        json!([])
    };
    let answer = json!({"type": "control_response", "response": {
        "subtype": "success", "request_id": REQUEST_ID,
        "response": {"rate_limits_available": true, "extra_usage": {"is_enabled": false},
            "rate_limits": {
                "five_hour": {"utilization": 10, "resets_at": reset.to_rfc3339()},
                "seven_day": {"utilization": 20, "resets_at": reset.to_rfc3339()},
                "model_scoped": models, "extra_usage": {"is_enabled": false}
            }}
    }});
    parse_answer(&answer.to_string(), now).unwrap()
}

#[test]
fn issue_3301_rotator_records_request_and_text_models_for_later_passive_decisions() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(ENV_CREDENTIAL_VARS);
    let name = ProfileName::parse("spare").unwrap();
    for mode in [ProbeMode::Auto, ProbeMode::Text] {
        let (_tmp, ctx, state) = setup();
        let now = chrono::Utc::now().timestamp();
        let reader = measure::reader(&ctx, env(mode), 1, now);
        let mut calls = 0;
        let direct = reader.profile_with(&name, false, |m| {
            calls += 1;
            assert_eq!(m, mode);
            Probe::read(report(m, now, true), Source::Fallback)
        });
        assert_eq!(calls, 1);
        assert_eq!(direct.report.week_models[0].used_percentage, 100.0);
        let stored = state.try_load_usage().unwrap().accounts["spare"]
            .week_models
            .clone();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].label, "Fable");

        // A newer reading with no model entry must not clear that observation.
        reader.profile_with(&name, false, |m| {
            Probe::read(report(m, now, false), Source::Fallback)
        });
        let before = fs::read(state.dir.join("usage.json")).unwrap();
        let reader = measure::reader(&ctx, env(ProbeMode::Ordered), 1, now + 1);
        let passive = reader.profile_with(&name, false, |_| panic!("expected stored usage"));
        assert_eq!(passive.source, Some(Source::Passive));
        let model = &passive.report.week_models[0];
        assert_eq!(model.used_percentage, 100.0);
        assert_eq!(model.observed_at, Some(stored[0].observed_at));
        assert_eq!(model.resets_at, stored[0].resets_at);
        assert!(matches!(
            measure::reading(name.as_str(), &passive).reading,
            Reading::Exhausted {
                week: Some(100.0),
                ..
            }
        ));
        assert_eq!(fs::read(state.dir.join("usage.json")).unwrap(), before);
    }
}

#[test]
fn issue_3301_decision_and_cycle_presentation_use_the_same_week_and_origin() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(ENV_CREDENTIAL_VARS);
    let name = ProfileName::parse("spare").unwrap();
    for mode in [ProbeMode::Auto, ProbeMode::Text] {
        for (all, second_model, origin) in [
            (20.0, None, Some("Fable")),
            (100.0, None, None),
            (20.0, Some("Another"), Some("Another")),
            (20.0, Some("Zed"), Some("Fable")),
        ] {
            let (_tmp, ctx, _state) = setup();
            let now = chrono::Utc::now().timestamp();
            let mut report = report(mode, now, true);
            let all_window = report.week_all.as_mut().unwrap();
            all_window.used_percentage = all;
            let all_reset = all_window.resets_at;
            report.week_models[0].resets_at = Some(now + 123);
            if let Some(label) = second_model {
                let mut model = report.week_models[0].clone();
                model.label = label.into();
                report.week_models.insert(0, model);
            }
            let reader = measure::reader(&ctx, env(mode), 1, now);
            let probe = reader.profile_with(&name, false, |_| {
                Probe::read(report.clone(), Source::Fallback)
            });
            let measured = measure::reading(name.as_str(), &probe);
            let deciding = measured.windows.deciding_week().unwrap();
            let shown = super::log::Shown::of(&measured);
            assert_eq!(deciding.used, 100.0);
            assert_eq!(deciding.model, origin);
            assert_eq!(shown.week, Some(deciding.used));
            assert_eq!(shown.week_model.as_deref(), deciding.model);
            assert!(matches!(
                measured.reading,
                Reading::Exhausted { week: Some(w), .. } if w == deciding.used
            ));
            assert_eq!(measured.windows.week.unwrap().resets_at, all_reset);
            assert_eq!(measured.windows.week.unwrap().used, all);
        }
    }
}

#[test]
fn issue_3301_rotator_rechecks_account_attribution_after_a_direct_read() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(ENV_CREDENTIAL_VARS);
    let (_tmp, ctx, state) = setup();
    let now = chrono::Utc::now().timestamp();
    let name = ProfileName::parse("spare").unwrap();
    let reader = measure::reader(&ctx, env(ProbeMode::Text), 1, now);
    reader.profile_with(&name, false, |m| {
        let mut ledger = state.load_ledger().unwrap();
        ledger.hosts.insert(
            "spare".into(),
            serde_json::from_value(json!({
                "live": "other", "swapped_at": now, "pre_swap_fingerprint": null
            }))
            .unwrap(),
        );
        state.save_ledger(&ledger).unwrap();
        Probe::read(report(m, now, true), Source::Fallback)
    });
    assert!(state.try_load_usage().unwrap().accounts.is_empty());
    reader.profile_with(&name, false, |m| {
        Probe::read(report(m, now, true), Source::Fallback)
    });
    let stored = state.try_load_usage().unwrap();
    assert!(!stored.accounts.contains_key("spare"));
    assert_eq!(
        stored.accounts["other"].week_models[0].used_percentage,
        100.0
    );
}

#[test]
fn issue_3301_oauth_read_records_models_unless_the_rotator_is_read_only() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(ENV_CREDENTIAL_VARS);
    for read_only in [false, true] {
        let (_tmp, mut ctx, state) = setup();
        ctx.read_only = read_only;
        let now = chrono::Utc::now().timestamp();
        let reset = chrono::DateTime::from_timestamp(now + 86400, 0)
            .unwrap()
            .to_rfc3339();
        let fake = FakeHttp::start(move |_, _| {
            Reply::json(
                200,
                &json!({
                    "five_hour": {"utilization": 10, "resets_at": reset},
                    "seven_day": {"utilization": 20, "resets_at": reset},
                    "model_scoped": [{"display_name": "Fable", "utilization": 100, "resets_at": reset}],
                    "extra_usage": {"is_enabled": false}
                }),
            )
        });
        let path = state.root.join("spare/.credentials.json");
        fs::write(
            &path,
            json!({"claudeAiOauth": {
                "accessToken": "FAKE-ACCESS-3301", "expiresAt": (now + 3600) * 1000
            }})
            .to_string(),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut direct = env(ProbeMode::Direct);
        direct.endpoints = Endpoints::resolve(
            Some(OsStr::new(&fake.base())),
            Some(OsStr::new(&fake.token_url())),
        )
        .0;
        let reader = measure::reader(&ctx, direct, 1, now);
        let p = reader.profile_with(&ProfileName::parse("spare").unwrap(), false, |_| {
            panic!("expected direct usage")
        });
        assert_eq!(p.source, Some(Source::Direct));
        assert_eq!(p.report.week_models[0].used_percentage, 100.0);
        let stored = state.try_load_usage().unwrap();
        if read_only {
            assert!(stored.accounts.is_empty());
        } else {
            assert_eq!(
                stored.accounts["spare"].week_models[0].used_percentage,
                100.0
            );
        }
    }
}
