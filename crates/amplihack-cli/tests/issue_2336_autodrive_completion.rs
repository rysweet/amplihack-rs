//! TDD contract for issue #2336: completed review objectives precede convergence
//! evaluation. These tests intentionally fail until the production Rust helper
//! and both Crusty entry points implement the completion-first contract.
//!
//! No live agent or GitHub call is made. The actual YAML bash commands, shared
//! loop, follow-up tools, state helpers and Rust helper binary are exercised.
//! Stubbed judgement does not establish real-agent instruction-following; that
//! remains a separate controlled validation during implementation delivery.

#[path = "issue_2336_autodrive_completion_failures.rs"]
mod failures;
#[path = "support/autodrive_completion.rs"]
pub(crate) mod fixture;
#[path = "issue_2336_autodrive_completion_live.rs"]
mod live;
#[path = "issue_2336_autodrive_completion_regressions.rs"]
mod regressions;
#[path = "issue_2333_rest_head_read.rs"]
mod rest_head_read;
#[path = "issue_2336_autodrive_completion_security.rs"]
mod security;

use fixture::{Drive, ROUND, assert_complete, json_output};
use serde_json::json;

use crate::support::auto_drive_light::{private_file, recipe, show, stdout};

fn done_record(d: &Drive) {
    d.record(1);
    private_file(
        &d.p("crusty-result.json"),
        &json!({
            "loop":"crusty", "loop_result":"DONE", "round_label":"round-1",
            "round_verdict":"MINOR_ONLY", "source":"objective-completion", "evaluator_invoked":false
        })
        .to_string(),
    );
}

#[test]
fn next_action_agent_runs_after_record_creation_and_states_the_completion_contract() {
    let yaml = recipe(ROUND);
    let steps = yaml["steps"].as_sequence().unwrap();
    let record = steps
        .iter()
        .position(|s| s["id"] == "step-06-write-round-record")
        .unwrap();
    let decision = steps
        .iter()
        .enumerate()
        .skip(record + 1)
        .find(|(_, s)| {
            s["agent"].is_string()
                && s["prompt"]
                    .as_str()
                    .is_some_and(|p| p.contains("complete") && p.contains("evaluate"))
        })
        .expect("a next-action agent must assess objective completion after the record is written");
    assert_eq!(
        decision.0,
        steps.len() - 1,
        "no remediation or evidence mutation follows next-action judgment"
    );
    let prompt = decision.1["prompt"].as_str().unwrap();
    for requirement in [
        "MINOR_ONLY",
        "CLEAN",
        "blocking",
        "major",
        "untrusted",
        "history",
        "79",
        "follow-up",
    ] {
        assert!(
            prompt.contains(requirement),
            "next-action instructions must cover {requirement}"
        );
    }
    assert!(
        prompt.contains("autodrive-completion-write"),
        "agent uses the Rust writer, never hand-crafted JSON"
    );
}

/// The goal loop's review action finished one round: it wrote `reviewed`, ran
/// exactly one Crusty round for turn 001 and launched no evaluator.
fn assert_reviewed(d: &Drive, o: &std::process::Output, result: &serde_json::Value) {
    assert!(o.status.success(), "{}", show(o));
    assert_eq!(result["outcome"], "reviewed", "{result} {}", show(o));
    assert_eq!(d.rounds(), ["turn-001"], "{}", show(o));
    assert!(
        d.evaluators().is_empty(),
        "a review never launches an evaluator: {}",
        show(o)
    );
}

/// #3129: the Crusty loop that exited on a completed MINOR_ONLY round, wrote
/// its manifest, progress and usage rows and then filed the follow-ups is
/// gone; the goal loop's review action runs the round. What stays is the
/// round's own record: the measured head, the MINOR_ONLY verdict and the
/// `.minor` sidecar, with no evaluator.
#[test]
fn fresh_minor_only_review_records_the_measured_head_without_an_evaluator() {
    let d = Drive::new();
    let (o, result, record_path) = d.review_turn();
    assert_reviewed(&d, &o, &result);
    assert!(
        result["summary"]
            .as_str()
            .is_some_and(|s| s.contains("Crusty verdict MINOR_ONLY")),
        "{result}"
    );
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap()).unwrap();
    assert_eq!(record["reviewed_head_sha"], d.head());
    assert_eq!(record["crusty_verdict"], "MINOR_ONLY");
    assert!(record_path.with_extension("json.minor").is_file());
}

/// #3129: the STUCK stop this guarded against went with the Crusty loop. The
/// review action still runs one new round beside six persisted MINOR_ONLY
/// rounds, launches no evaluator and leaves the earlier records unchanged.
#[test]
fn six_persisted_minor_only_rounds_are_kept_when_the_review_runs_again() {
    let d = Drive::new();
    for n in 1..=6 {
        d.record(n);
    }
    let preserved: Vec<_> = (1..=6)
        .map(|n| d.read(&format!("crusty-round-{n}.json")))
        .collect();
    std::fs::write(d.fakes.log.join("rounds"), "").unwrap();
    let (o, result, _) = d.review_turn();
    assert_reviewed(&d, &o, &result);
    for (n, original) in (1..=6).zip(preserved) {
        assert_eq!(
            d.read(&format!("crusty-round-{n}.json")),
            original,
            "resume preserves historical evidence"
        );
    }
}

#[test]
fn six_minor_only_results_in_same_run_history_are_complete_at_dispatch_boundary() {
    let d = Drive::new();
    // Replay legacy history at dispatch. Forcing six completed rounds through
    // the corrected loop would contradict the very behavior being specified.
    d.record(7);
    let history = (1..=6)
        .map(|n| format!("round-{n}: crusty_verdict=MINOR_ONLY rc=0 findings=0\n"))
        .collect::<String>();
    let p = d.prepare(7, &history);
    d.decide(&p, "complete");
    assert_complete(&d, &d.dispatch(&p, 0));
}

#[test]
fn clean_completion_preserves_reviewed_head_without_evaluation() {
    let d = Drive::new();
    d.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);
    d.record(1);
    let p = d.prepare(1, "");
    d.decide(&p, "complete");
    assert_complete(&d, &d.dispatch(&p, 0));
}

#[test]
fn unresolved_major_and_blocking_concerns_still_reach_evaluator_at_sibling_depth() {
    for severity in ["major", "blocking"] {
        let d = Drive::new();
        d.set("review.json", &json!({"crusty_verdict":"CONCERNS", "concerns":[{
            "id":"wrong-result", "severity":severity, "summary":"Wrong return value", "evidence":"lib.rs:4"
        }]}).to_string());
        d.record(1);
        let p = d.prepare(1, "round-0: crusty_verdict=CONCERNS rc=0");
        d.decide(&p, "evaluate");
        let o = d.dispatch(&p, 0);
        assert!(
            !o.status.success(),
            "evaluator STUCK remains a failure: {}",
            show(&o)
        );
        let v = json_output(&o);
        assert_eq!(v["loop_result"], "STUCK");
        assert_eq!(v["source"], "evaluator");
        assert_eq!(v["evaluator_invoked"], true);
        let calls = d.evaluators();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["depth"], "2");
        assert_eq!(calls[0]["ceiling"], "6");
        let argv = calls[0]["argv"].as_array().unwrap();
        assert!(
            argv.iter().any(|v| v == &json!(d.repo)),
            "evaluator works in the actual PR worktree"
        );
        assert!(argv.iter().any(|v| v == "loop_round_verdict=CONCERNS"));
        assert!(!d.marked());
    }
}

#[test]
fn stale_phase_marker_cannot_authorize_new_review_with_unrecorded_minors() {
    let d = Drive::new();
    done_record(&d);
    let o = d.finalize(false, false);
    assert!(o.status.success() && d.marked(), "baseline: {}", show(&o));
    // A later review adds a mandatory minor but has not recorded it. Even if
    // the head is unchanged, an old phase marker is not authority for it.
    d.set("review.json", r#"{"crusty_verdict":"CONCERNS","concerns":[{"id":"new-minor","severity":"minor","summary":"New typo","evidence":"README.md:5"}]}"#);
    d.record(2);
    let o = d.script(". \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_state.sh\"; autodrive_crusty_final \"$STATE\"")
        .env("STATE", &d.state).output().unwrap();
    assert!(
        !o.status.success(),
        "old marker must be bound to its own review evidence: {}",
        show(&o)
    );
}

#[test]
fn terminal_children_cannot_claim_completion_or_trigger_evaluation() {
    for rc in [79, 75, 1] {
        let d = Drive::new();
        d.record(1);
        let p = d.prepare(1, "");
        d.decide(&p, "complete");
        let o = d.dispatch(&p, rc);
        assert!(
            !o.status.success(),
            "failed child cannot complete: {}",
            show(&o)
        );
        if rc == 79 {
            assert_eq!(o.status.code(), Some(79), "terminal refusal must propagate");
        }
        if rc == 75 {
            assert_eq!(o.status.code(), Some(75), "orphaning must propagate");
        }
        assert!(
            d.evaluators().is_empty(),
            "invalid completion is never retried through evaluation"
        );
        assert!(!d.marked());
    }
}

#[test]
fn helper_does_not_forge_a_completed_result_for_an_unresolved_record() {
    let d = Drive::new();
    d.set("review.json", r#"{"crusty_verdict":"CONCERNS","concerns":[{"id":"wrong-value","severity":"major","summary":"Wrong value","evidence":"lib.rs:2"}]}"#);
    d.record(1);
    let p = d.prepare(1, "");
    d.decide(&p, "complete");
    let o = d.dispatch(&p, 0);
    assert!(
        !o.status.success(),
        "completion claim needs allowed round evidence: {}",
        show(&o)
    );
    assert!(d.evaluators().is_empty());
    assert!(!stdout(&o).contains("objective-completion"));
}

/// #2814: the record carries the commit the loop measured, and recording it is
/// all the loop does with it. A review that names another commit leaves no
/// finding, refuses nothing and stops nothing: the round completes on its
/// verdict exactly as it would have had the review named the same commit. The
/// reconciliation this replaces turned that round into a stopped drive.
/// #3129: the goal loop's review action measures the head and runs the round,
/// as the Crusty loop did.
#[test]
fn a_review_naming_another_commit_does_not_change_the_round() {
    let d = Drive::new();
    d.git(&d.repo, &["commit", "--allow-empty", "-qm", "other"]);
    let other = d.head();
    d.git(&d.repo, &["reset", "-q", "--hard", "HEAD~1"]);
    assert_ne!(other, d.head());
    d.set("review-head", &other);
    let (o, result, record_path) = d.review_turn();
    assert_reviewed(&d, &o, &result);
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap()).unwrap();
    assert_eq!(
        record["reviewed_head_sha"],
        d.head(),
        "the record carries the measured commit, not the claim: {}",
        show(&o)
    );
    assert_eq!(
        std::fs::read_to_string(record_path.with_extension("json.findings")).unwrap(),
        "",
        "a disagreeing claim is not a finding: {}",
        show(&o)
    );
}
