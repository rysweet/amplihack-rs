//! Issue #2539: recovery stops looping on evidence, not on a count.
//!
//! A failure is a recurrence when an earlier record for the same step has the
//! same signature and chose `retry_after_repair` or `alternative_step`. The
//! driver computes it, across separate `run` calls sharing one directory (an
//! auto-drive loop's rounds), and refuses a retry or the same alternative.

#![cfg(unix)]

use std::fs;

use serde_json::{Value, json};

use crate::support::step_recovery::{Fixture, HANDOFF_EXIT, outcome, show};

/// A tool that always fails the same way, with a different PID in its output
/// each time, and logs each run to `runs.log`.
fn same_failure(f: &Fixture) -> String {
    let log = f.root.join("runs.log");
    f.tool(
        "locked",
        &format!(
            "#!/bin/sh\necho run >> '{}'\necho \"index.lock held by pid $$ at 2026-11-12T21:04:33Z (ref 9d521815af)\" >&2\nexit 1\n",
            log.display()
        ),
    )
    .to_string_lossy()
    .into_owned()
}

fn runs(f: &Fixture) -> usize {
    fs::read_to_string(f.root.join("runs.log"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn allowed(evidence: &Value) -> Vec<String> {
    evidence["allowed_choices"]
        .as_array()
        .expect("allowed_choices")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn retry(why: &str) -> Value {
    json!({"choice": "retry_after_repair", "reason": why, "repair": "removed the stale index.lock"})
}

#[test]
fn a_repair_that_fails_the_same_way_ends_in_the_agents_handoff() {
    let f = Fixture::new();
    let tool = same_failure(&f);
    f.choose(1, retry("stale lock from a killed git"));
    f.choose(
        2,
        json!({"choice": "handoff", "reason": "the lock comes back after removal",
        "owner": "run-requester", "needs": "find the process that keeps taking index.lock"}),
    );

    let o = f.run("step-03-finalize", &[], &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(outcome(&o)["choice"], "handoff");
    assert_eq!(
        runs(&f),
        2,
        "the command ran once, then once after the repair"
    );

    let records = f.records("step-03-finalize");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["recurrence"], false);
    assert_eq!(records[1]["recurrence"], true);
    assert_eq!(records[1]["source"], "agent");
    assert_eq!(
        records[0]["failure"]["signature"], records[1]["failure"]["signature"],
        "different PIDs, times and commit ids give the same signature"
    );

    let second = f.evidence(2);
    assert_eq!(second["recurrence"], true);
    assert!(
        !allowed(&second).contains(&"retry_after_repair".to_string()),
        "{second}"
    );
    assert_eq!(second["prior_records"].as_array().unwrap().len(), 1);
    assert_eq!(f.report().status.code(), Some(HANDOFF_EXIT));
}

#[test]
fn a_retry_chosen_again_on_a_recurrence_becomes_a_validator_handoff() {
    let f = Fixture::new();
    let tool = same_failure(&f);
    f.choose(1, retry("stale lock from a killed git"));
    f.choose(2, retry("AGENT-SECOND-REASON try once more"));

    let o = f.run("step-03-finalize", &[], &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(runs(&f), 2, "the refused retry never runs the command");
    assert_eq!(f.agent_calls().len(), 2);

    let records = f.records("step-03-finalize");
    let last = records.last().unwrap();
    assert_eq!(last["choice"], "handoff");
    assert_eq!(last["source"], "validator");
    assert_eq!(last["recurrence"], true);
    assert_eq!(last["owner"], "run-requester");
    assert!(
        last["reason"]
            .as_str()
            .unwrap()
            .contains("AGENT-SECOND-REASON"),
        "the validator keeps the agent's reason: {last}"
    );
    assert_eq!(outcome(&o)["source"], "validator");
}

#[test]
fn a_failure_repaired_in_one_round_is_a_recurrence_in_a_later_round() {
    let f = Fixture::new();
    let flag = f.root.join("fixed-once");
    let tool = f.tool(
        "flaky-lock",
        &format!(
            "#!/bin/sh\nif [ -f '{0}' ]; then rm -f '{0}'; echo merged; exit 0; fi\necho \"index.lock held by pid $$\" >&2\nexit 1\n",
            flag.display()
        ),
    );
    f.repair(1, &format!("#!/bin/sh\ntouch '{}'\n", flag.display()));
    f.choose(1, retry("stale lock"));
    let round1 = f.run("step-02-merge-ready-loop", &[], &[tool.to_str().unwrap()]);
    assert_eq!(round1.status.code(), Some(0), "{}", show(&round1));
    assert!(String::from_utf8_lossy(&round1.stdout).contains("merged"));

    // Round 7: the same failure, from a fresh `run` sharing the directory.
    f.choose(2, retry("stale lock again"));
    let round7 = f.run("step-02-merge-ready-loop", &[], &[tool.to_str().unwrap()]);
    assert_eq!(round7.status.code(), Some(0), "{}", show(&round7));
    assert!(!allowed(&f.evidence(2)).contains(&"retry_after_repair".to_string()));
    assert_eq!(f.evidence(2)["recurrence"], true);
    let records = f.records("step-02-merge-ready-loop");
    assert_eq!(records.len(), 2);
    assert_eq!(
        (records[1]["choice"].as_str(), records[1]["source"].as_str()),
        (Some("handoff"), Some("validator"))
    );
}

#[test]
fn the_same_alternative_on_a_recurrence_is_refused_but_another_is_allowed() {
    let f = Fixture::new();
    let tool = same_failure(&f);
    let ok = f.tool("rest-read", "#!/bin/sh\necho head-from-rest\n");
    let alts = [
        "--alternative".to_string(),
        format!("rest={}", ok.display()),
        "--alternative".to_string(),
        format!("cached={}", ok.display()),
    ];
    let alts: Vec<&str> = alts.iter().map(String::as_str).collect();
    let alt = |name: &str| json!({"choice": "alternative_step", "reason": "read it another way", "alternative": name});

    f.choose(1, alt("rest"));
    assert_eq!(
        f.run("step-02-merge-ready-loop", &alts, &[&tool])
            .status
            .code(),
        Some(0)
    );
    // A later round: same failure, a different declared alternative is fine.
    f.choose(2, alt("cached"));
    let o = f.run("step-02-merge-ready-loop", &alts, &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("head-from-rest"));
    // And again: `rest` was already chosen for this signature (check 7).
    f.choose(3, alt("rest"));
    let o = f.run("step-02-merge-ready-loop", &alts, &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));

    let records = f.records("step-02-merge-ready-loop");
    assert_eq!(records.len(), 3);
    assert_eq!(records[1]["choice"], "alternative_step");
    assert_eq!(records[1]["recurrence"], true);
    assert_eq!(
        (records[2]["choice"].as_str(), records[2]["source"].as_str()),
        (Some("handoff"), Some("validator"))
    );
}

#[test]
fn a_different_failure_is_not_a_recurrence() {
    let f = Fixture::new();
    let state = f.root.join("which");
    let tool = f.tool(
        "two-faces",
        &format!(
            "#!/bin/sh\nif [ -f '{0}' ]; then echo 'error: remote rejected the push' >&2; else touch '{0}'; echo 'error: index.lock exists' >&2; fi\nexit 1\n",
            state.display()
        ),
    );
    f.choose(1, retry("stale lock"));
    f.choose(2, retry("a new failure after the repair"));
    f.choose(
        3,
        json!({"choice": "continue_recorded", "reason": "push cleanup is not essential"}),
    );

    let o = f.run("step-20b-push-cleanup", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let records = f.records("step-20b-push-cleanup");
    assert_ne!(
        records[0]["failure"]["signature"],
        records[1]["failure"]["signature"]
    );
    assert_eq!(records[1]["recurrence"], false);
    assert_eq!(
        records[1]["choice"], "retry_after_repair",
        "a new failure may be retried"
    );
    assert_eq!(
        records[2]["recurrence"], true,
        "the second message came back after its repair"
    );
    assert_eq!(records[2]["choice"], "continue_recorded");
}

#[test]
fn once_a_handoff_is_recorded_no_further_agent_starts_in_the_run() {
    let f = Fixture::new();
    let record = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "record", "--dir"])
        .arg(&f.dir)
        .args([
            "--step-id",
            "autodrive-report",
            "--reason",
            "no finalization evidence",
            "--owner",
            "run-requester",
            "--needs",
            "rerun finalization",
        ])
        .output()
        .unwrap();
    assert_eq!(record.status.code(), Some(0), "{}", show(&record));

    let tool = same_failure(&f);
    f.choose(1, retry("should never be read"));
    let o = f.run("step-03-finalize", &[], &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(f.agent_calls().is_empty(), "the run is already handed off");
    let last = f.records("step-03-finalize").pop().unwrap();
    assert_eq!(
        (last["choice"].as_str(), last["source"].as_str()),
        (Some("handoff"), Some("validator"))
    );
    let reason = last["reason"].as_str().unwrap();
    assert!(
        reason.contains("autodrive-report"),
        "names the earlier handoff: {reason}"
    );
}
