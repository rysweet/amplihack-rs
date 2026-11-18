//! Round publication keeps existing completion evidence without review folders.
use super::fixture::{Fixture, succeeds, workspace};
use serde_json::json;
use std::fs;

#[test]
fn round_records_keep_reviewed_and_resulting_heads_without_generation_binding() {
    for made_commit in [false, true] {
        let f = Fixture::new();
        if made_commit {
            f.git(
                &f.pr,
                &["commit", "--allow-empty", "-qm", "address review concern"],
            );
        }
        let resulting_head = f.git_text(&f.pr, &["rev-parse", "HEAD"]);
        let record = f.state.join("crusty-round-1.json");
        let verdict = if made_commit { "CONCERNS" } else { "CLEAN" };
        let out = f
            .command("bash")
            .current_dir(&f.pr)
            .arg(workspace().join("amplifier-bundle/tools/autodrive_crusty_record.sh"))
            .env("AUTODRIVE_ROUND_RECORD", &record)
            .env("AUTODRIVE_ROUND_LABEL", "round-1")
            .env(
                "RECIPE_VAR_crusty_verdict",
                json!({"crusty_verdict":verdict,"concern_count":u8::from(made_commit)}).to_string(),
            )
            .env(
                "RECIPE_VAR_crusty_round_context",
                json!({"pr":"2808","head_sha":f.observed_head}).to_string(),
            )
            .env(
                "RECIPE_VAR_crusty_fix_evidence",
                json!({"commits":u8::from(made_commit),"head_sha":resulting_head}).to_string(),
            )
            .output()
            .unwrap();
        succeeds(&out);
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        assert_eq!(value["reviewed_head_sha"], f.observed_head);
        assert_eq!(value["head_sha"], resulting_head);
        assert_eq!(value["commits_this_round"], u8::from(made_commit));
        assert_eq!(value["crusty_verdict"], verdict);
        assert!(value.get("review_generation").is_none());
        // Neither optional sidecar is needed to record a CLEAN review or a
        // fix round with a different resulting head. Existing completion tests
        // remain responsible for the writer's sealed complete/evaluate policy.
        assert!(!record.with_extension("json.minor").exists());
        assert!(!record.with_extension("json.severity").exists());
    }
}

#[test]
fn record_write_failure_is_reported_without_claiming_success() {
    let f = Fixture::new();
    let out = f
        .command("bash")
        .arg(workspace().join("amplifier-bundle/tools/autodrive_crusty_record.sh"))
        .env("AUTODRIVE_ROUND_RECORD", f.state.join("missing/round.json"))
        .env("REPO_PATH", &f.pr)
        .env(
            "RECIPE_VAR_crusty_verdict",
            json!({"crusty_verdict":"CLEAN","concern_count":0}).to_string(),
        )
        .env(
            "RECIPE_VAR_crusty_round_context",
            json!({"head_sha":f.observed_head}).to_string(),
        )
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "failed record I/O must fail the writer: {out:?}"
    );
    assert!(!f.state.join("missing/round.json").exists());
    assert!(out.stdout.is_empty(), "no record was published: {out:?}");
}
