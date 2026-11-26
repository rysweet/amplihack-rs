//! CLI scenarios with temporary profiles, fake probes and loopback endpoints.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::{Fixture, email, text};
use crate::support::wait::output_within;
use serde_json::{Value, json};
use std::fs;
use std::process::Command;

fn execute(cmd: &mut Command) -> Value {
    let out = output_within("isolated model-week scenario", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn row(rows: &Value) -> &Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["profile"] == "spare")
        .unwrap()
}

fn model_text(f: &Fixture, used: u32) {
    f.usage_resetting("spare", 10, 20, 3);
    let path = f.canned.join(format!("{}.out", email("spare")));
    let mut body = fs::read_to_string(&path).unwrap();
    let reset = (chrono::Utc::now() + chrono::Duration::hours(3))
        .format("%b %-d, %-I:%M%P (UTC)")
        .to_string();
    body.push_str(&format!(
        "Current week (Fable): {used}% used · resets {reset}\n"
    ));
    fs::write(path, body).unwrap();
}

fn request_answer(f: &Fixture) {
    let reset = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
    let answer = json!({"type": "control_response", "response": {
        "subtype": "success", "request_id": "amplihack-usage",
        "response": {"rate_limits_available": true, "extra_usage": {"is_enabled": false},
            "rate_limits": {
                "five_hour": {"utilization": 10, "resets_at": reset},
                "seven_day": {"utilization": 20, "resets_at": reset},
                "model_scoped": [{"display_name": "Fable", "utilization": 100,
                    "resets_at": reset}],
                "extra_usage": {"is_enabled": false}
            }}
    }});
    fs::write(
        f.canned.join(format!("{}.json", email("spare"))),
        answer.to_string(),
    )
    .unwrap();
}

#[test]
fn issue_3301_cli_direct_sources_survive_passive_reads_without_reageing() {
    for mode in ["auto", "text"] {
        let f = Fixture::new();
        model_text(&f, 100);
        request_answer(&f);
        let direct = execute(
            f.cmd(&["profile", "usage", "--json"])
                .env("AMPLIHACK_PROFILE_USAGE_PROBE", mode),
        );
        let observed = row(&direct)["week_models"][0].clone();
        assert_eq!(observed["used_percentage"], 100.0);
        assert!(observed["resets_at"].is_i64());
        let path = f.root.join(".auto-switch.d/usage.json");
        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let recorded = stored["accounts"]["spare"]["week_models"][0].clone();
        assert!(recorded["observed_at"].is_i64());

        // A later direct observation omits this model. Omission is not a reset.
        f.usage_resetting("spare", 10, 20, 3);
        fs::remove_file(f.canned.join(format!("{}.json", email("spare")))).unwrap();
        execute(
            f.cmd(&["profile", "usage", "--json"])
                .env("AMPLIHACK_PROFILE_USAGE_PROBE", "text"),
        );
        let before = fs::read(&path).unwrap();
        let passive = execute(
            f.cmd(&["profile", "usage", "--json"])
                .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600"),
        );
        assert_eq!(row(&passive)["source"], "passive");
        let retained = &row(&passive)["week_models"][0];
        for field in ["label", "used_percentage", "resets_at"] {
            assert_eq!(retained[field], observed[field]);
        }
        assert_eq!(retained["observed_at"], recorded["observed_at"]);
        assert_eq!(fs::read(&path).unwrap(), before);

        // Move the recorded reset into the past, without waiting on a clock.
        let mut stored: Value = serde_json::from_slice(&before).unwrap();
        stored["accounts"]["spare"]["week_models"][0]["resets_at"] =
            json!(chrono::Utc::now().timestamp() - 1);
        fs::write(&path, stored.to_string()).unwrap();
        let expired = execute(
            f.cmd(&["profile", "usage", "--json"])
                .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600"),
        );
        assert_eq!(row(&expired)["source"], "passive");
        assert_eq!(row(&expired)["week_models"], json!([]));
    }
}

#[test]
fn issue_3301_cli_confirms_one_passive_target_and_rejects_model_threshold_equality() {
    for used in [97, 98, 100] {
        let f = Fixture::new();
        model_text(&f, used);
        // Keep the confirmed target preferable below the model threshold.
        // A rejected target leaves an eligible alternative needing its own read.
        f.passive(&[("work", 100, 20), ("spare", 10, 20), ("extra", 50, 99)]);
        let before = f.usage_probes("spare");
        let out = output_within(
            "isolated target confirmation",
            f.cmd(&[
                "profile",
                "rotate",
                "--session-at",
                "95",
                "--week-at",
                "98",
                "--target-week-max",
                "100",
            ])
            .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600"),
        );
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_eq!(f.usage_probes("spare") - before, 1);
        assert_eq!(f.usage_probes("extra"), 0);
        assert_eq!(
            f.home_account(),
            if used < 98 { "uuid-spare" } else { "uuid-work" },
            "model={used}\n{}\n{}",
            text(&out.stdout),
            text(&out.stderr),
        );
        f.assert_each_login_once();
    }
}
