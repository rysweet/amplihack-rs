//! Round publication keeps existing completion evidence without review folders,
//! and the recorded reviewed head is the one the loop measured (#2814).
use super::fixture::{Fixture, succeeds, workspace};
use serde_json::json;
use std::fs;
use std::process::Output;

/// One round as the writer sees it. `measured` is the loop's own
/// `reviewed_head_sha` context value; `claim` is what the review reported.
struct Round<'a> {
    verdict: &'a str,
    concerns: u8,
    commits: u8,
    resulting_head: &'a str,
    measured: Option<&'a str>,
    claim: Option<&'a str>,
}

fn write_record(f: &Fixture, record: &std::path::Path, round: Round<'_>) -> Output {
    let mut command = f.command("bash");
    command
        .current_dir(&f.pr)
        .arg(workspace().join("amplifier-bundle/tools/autodrive_crusty_record.sh"))
        .env("AUTODRIVE_ROUND_RECORD", record)
        .env("AUTODRIVE_ROUND_LABEL", "round-1")
        .env(
            "RECIPE_VAR_crusty_verdict",
            json!({"crusty_verdict": round.verdict, "concern_count": round.concerns}).to_string(),
        )
        .env(
            "RECIPE_VAR_crusty_round_context",
            json!({"pr": "2808", "head_sha": f.observed_head}).to_string(),
        )
        .env(
            "RECIPE_VAR_crusty_fix_evidence",
            json!({"commits": round.commits, "head_sha": round.resulting_head}).to_string(),
        );
    if let Some(measured) = round.measured {
        command.env("RECIPE_VAR_reviewed_head_sha", measured);
    }
    if let Some(claim) = round.claim {
        command.env(
            "RECIPE_VAR_crusty_review",
            json!({ "reviewed_head_sha": claim }).to_string(),
        );
    }
    command.output().unwrap()
}

fn record_json(record: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(record).unwrap()).unwrap()
}

fn findings(record: &std::path::Path) -> String {
    fs::read_to_string(record.with_extension("json.findings")).unwrap()
}

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
        let out = write_record(
            &f,
            &record,
            Round {
                verdict,
                concerns: u8::from(made_commit),
                commits: u8::from(made_commit),
                resulting_head: &resulting_head,
                measured: Some(&f.observed_head),
                claim: Some(&f.observed_head),
            },
        );
        succeeds(&out);
        let value = record_json(&record);
        assert_eq!(value["reviewed_head_sha"], f.observed_head);
        assert_eq!(value["head_sha"], resulting_head);
        assert_eq!(value["commits_this_round"], u8::from(made_commit));
        assert_eq!(value["crusty_verdict"], verdict);
        // An agreeing review leaves no head finding behind, so a CLEAN round
        // can still complete.
        assert_eq!(findings(&record), "", "{out:?}");
        // Neither optional sidecar is needed to record a CLEAN review or a
        // fix round with a different resulting head. Existing completion tests
        // remain responsible for the writer's sealed complete/evaluate policy.
        assert!(!record.with_extension("json.minor").exists());
        assert!(!record.with_extension("json.severity").exists());
    }
}

/// #2814: the record carries the commit the loop measured, never the one the
/// review claimed, and a review naming another commit leaves an open finding
/// so the round goes to the evaluator instead of completing.
#[test]
fn a_review_naming_another_commit_is_recorded_against_the_loop_measured_head() {
    let f = Fixture::new();
    let other = f.git_text(&f.pr, &["rev-parse", "HEAD~1"]);
    assert_ne!(other, f.observed_head);
    let record = f.state.join("crusty-round-1.json");
    let out = write_record(
        &f,
        &record,
        Round {
            verdict: "CLEAN",
            concerns: 0,
            commits: 0,
            resulting_head: &f.observed_head,
            measured: Some(&f.observed_head),
            claim: Some(&other),
        },
    );
    // The check reports; it never fails the round by itself.
    succeeds(&out);
    let value = record_json(&record);
    assert_eq!(
        value["reviewed_head_sha"], f.observed_head,
        "the claim must not become the record's reviewed head: {out:?}"
    );
    assert!(
        findings(&record).contains("crusty-head-claim-mismatch"),
        "the disagreement must be an open finding: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("crusty-head-claim-mismatch"), "{stderr}");
    assert!(stderr.contains(&f.observed_head), "{stderr}");
}

/// #2814: a review that reports no `reviewed_head_sha` is a reportable
/// failure, not a silent fall back to the round context's head.
#[test]
fn a_review_without_a_reviewed_head_sha_is_reported_and_cannot_complete() {
    let f = Fixture::new();
    let record = f.state.join("crusty-round-1.json");
    let out = write_record(
        &f,
        &record,
        Round {
            verdict: "CLEAN",
            concerns: 0,
            commits: 0,
            resulting_head: &f.observed_head,
            measured: Some(&f.observed_head),
            claim: None,
        },
    );
    succeeds(&out);
    assert_eq!(record_json(&record)["reviewed_head_sha"], f.observed_head);
    assert!(
        findings(&record).contains("crusty-head-claim-missing"),
        "{out:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("crusty-head-claim-missing"),
        "{out:?}"
    );
}

/// #2814: without the loop's own measurement there is nothing to check the
/// claim against, so the round may not complete on it either.
#[test]
fn a_round_with_no_loop_measured_head_cannot_complete_on_the_claim_alone() {
    let f = Fixture::new();
    let record = f.state.join("crusty-round-1.json");
    let out = write_record(
        &f,
        &record,
        Round {
            verdict: "CLEAN",
            concerns: 0,
            commits: 0,
            resulting_head: &f.observed_head,
            measured: None,
            claim: Some(&f.observed_head),
        },
    );
    succeeds(&out);
    assert_eq!(record_json(&record)["reviewed_head_sha"], f.observed_head);
    assert!(
        findings(&record).contains("crusty-head-unmeasured"),
        "{out:?}"
    );
}

#[test]
fn record_write_failure_is_reported_without_claiming_success() {
    let f = Fixture::new();
    let out = f
        .command("bash")
        .arg(workspace().join("amplifier-bundle/tools/autodrive_crusty_record.sh"))
        .env("AUTODRIVE_ROUND_RECORD", f.state.join("missing/round.json"))
        .env("REPO_PATH", &f.pr)
        .env("RECIPE_VAR_reviewed_head_sha", &f.observed_head)
        .env(
            "RECIPE_VAR_crusty_verdict",
            json!({"crusty_verdict":"CLEAN","concern_count":0}).to_string(),
        )
        .env(
            "RECIPE_VAR_crusty_review",
            json!({"reviewed_head_sha": f.observed_head}).to_string(),
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
