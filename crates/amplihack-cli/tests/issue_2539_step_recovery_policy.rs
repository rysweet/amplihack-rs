//! Issue #2539: the paths a wrapped step takes without an agent's choice
//! deciding them (success, exit 79, exit 81, a signal) and the two shapes a
//! recovery takes that are not plain bash steps (`--already-failed` for
//! sub-recipe steps, and a failed alternative).

#![cfg(unix)]

use serde_json::json;

use crate::support::step_recovery::{Fixture, HANDOFF_EXIT, outcome, show, text};

#[test]
fn success_passes_output_through_unchanged_with_no_record_and_no_agent() {
    let f = Fixture::new();
    let tool = f.tool(
        "chatty",
        "#!/bin/sh\ni=0\nwhile [ $i -lt 5000 ]; do echo \"line $i\"; i=$((i+1)); done\n",
    );
    let o = f.run("step-03-finalize", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let expected: String = (0..5000).map(|i| format!("line {i}\n")).collect();
    assert_eq!(
        text(&o.stdout),
        expected,
        "stdout is the command's, byte for byte"
    );
    assert!(f.agent_calls().is_empty());
    assert!(
        !f.dir_entries().iter().any(|n| n.ends_with(".jsonl")),
        "{:?}",
        f.dir_entries()
    );
}

#[test]
fn exit_79_is_a_policy_handoff_and_79_is_passed_on_without_an_agent() {
    let f = Fixture::new();
    let tool = f.tool(
        "refused",
        "#!/bin/sh\necho 'BLOCKED_TERMINAL depth guard' >&2\nexit 79\n",
    );
    let o = f.run("step-03-finalize", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
    assert!(f.agent_calls().is_empty(), "exit 79 never reaches an agent");
    let records = f.records("step-03-finalize");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["choice"], "handoff");
    assert_eq!(records[0]["source"], "policy");
    assert_eq!(records[0]["reason"], "terminal policy refusal (exit 79)");
    assert_eq!(records[0]["failure"]["exit_code"], 79);
    assert!(
        !f.dir_entries().iter().any(|n| n.starts_with("evidence-")),
        "no evidence is built"
    );
    assert_eq!(f.report().status.code(), Some(HANDOFF_EXIT));
}

#[test]
fn exit_81_from_a_nested_run_is_passed_on_with_no_new_record_and_no_agent() {
    let f = Fixture::new();
    // The nested run recorded its handoff in the shared directory first.
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "record", "--dir"])
        .arg(&f.dir)
        .args([
            "--step-id",
            "step-21-pr-ready",
            "--reason",
            "nested handoff",
        ])
        .args(["--owner", "run-requester", "--needs", "mark the PR ready"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let tool = f.tool("nested-run", "#!/bin/sh\nexit 81\n");
    let o = f.run("step-02-default-workflow", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(81), "{}", show(&o));
    assert!(f.agent_calls().is_empty());
    let written = f.dir_entries();
    assert!(
        !written
            .iter()
            .any(|n| n.starts_with("evidence-") || n == "step-02-default-workflow.jsonl"),
        "no record and no evidence: {written:?}"
    );
}

#[test]
fn exit_81_with_no_handoff_record_is_an_ordinary_failure() {
    let f = Fixture::new();
    let tool = f.tool("fake-handoff", "#!/bin/sh\nexit 81\n");
    f.choose(
        1,
        json!({"choice": "handoff", "reason": "exit 81 with no record",
        "owner": "run-requester", "needs": "check the step"}),
    );
    let o = f.run(
        "step-02-default-workflow",
        &["--stop-on-handoff"],
        &[tool.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(81), "{}", show(&o));
    assert_eq!(f.agent_calls().len(), 1, "the failure went to recovery");
    let records = f.records("step-02-default-workflow");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["failure"]["exit_code"], 81);
    assert_eq!(records[0]["source"], "agent");
}

#[test]
fn a_command_killed_by_sigterm_exits_143_with_a_policy_record_and_no_agent() {
    let f = Fixture::new();
    let tool = f.tool("cancelled", "#!/bin/sh\nkill -TERM $$\n");
    let o = f.run("step-02-crusty-loop", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(143), "{}", show(&o));
    assert!(
        f.agent_calls().is_empty(),
        "a cancelled run never begins a repair"
    );
    let records = f.records("step-02-crusty-loop");
    assert_eq!(records[0]["source"], "policy");
    assert_eq!(records[0]["choice"], "handoff");
    assert_eq!(records[0]["failure"]["signal"], 15);
    assert!(records[0]["failure"]["exit_code"].is_null());
}

#[test]
fn already_failed_goes_straight_to_recovery_and_records_no_invented_status() {
    let f = Fixture::new();
    let marker = f.root.join("ran");
    let again = f.tool(
        "build-again",
        &format!(
            "#!/bin/sh\necho built > '{}'\necho build-ok\n",
            marker.display()
        ),
    );
    f.choose(
        1,
        json!({"choice": "retry_after_repair", "reason": "the sub-recipe stopped early",
        "repair": "cleared the stale worktree lock"}),
    );

    let o = f.run(
        "step-02-default-workflow",
        &["--already-failed", "--essential"],
        &[again.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(text(&o.stdout).contains("build-ok"), "{}", show(&o));
    assert!(marker.exists(), "the retry runs the command after --");

    let failure = &f.records("step-02-default-workflow")[0]["failure"];
    assert!(failure["exit_code"].is_null());
    assert_eq!(
        failure["unavailable_reason"],
        "runner does not expose failed sub-recipe status"
    );
    assert!(
        failure["signature"]
            .as_str()
            .unwrap()
            .starts_with("exit=none;")
    );

    let allowed = f.evidence(1)["allowed_choices"].clone();
    assert!(
        !allowed
            .as_array()
            .unwrap()
            .contains(&json!("continue_recorded")),
        "--essential: {allowed}"
    );
    assert!(
        !allowed
            .as_array()
            .unwrap()
            .contains(&json!("alternative_step")),
        "none declared: {allowed}"
    );
}

#[test]
fn a_failed_alternative_is_recovered_under_its_own_id_without_alternatives() {
    let f = Fixture::new();
    let bad = f.tool("bad", "#!/bin/sh\necho 'primary failed' >&2\nexit 3\n");
    let alt = f.tool("alt-bad", "#!/bin/sh\necho 'fallback failed' >&2\nexit 4\n");
    f.choose(1, json!({"choice": "alternative_step", "reason": "try the fallback", "alternative": "fallback"}));
    f.choose(
        2,
        json!({"choice": "handoff", "reason": "the fallback failed too",
        "owner": "run-requester", "needs": "look at both failures"}),
    );

    let declared = format!("fallback={}", alt.display());
    let o = f.run(
        "step-20b-push-cleanup",
        &["--alternative", &declared],
        &[bad.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(outcome(&o)["step_id"], "step-20b-push-cleanup:alt:fallback");

    let alt_records = f.records("step-20b-push-cleanup__alt__fallback");
    assert_eq!(
        alt_records[0]["step_id"],
        "step-20b-push-cleanup:alt:fallback"
    );
    assert_eq!(alt_records[0]["failure"]["exit_code"], 4);
    let allowed = f.evidence(2)["allowed_choices"].clone();
    assert!(
        !allowed
            .as_array()
            .unwrap()
            .contains(&json!("alternative_step")),
        "{allowed}"
    );
    assert_eq!(f.evidence(2)["alternatives"], json!([]));
}

/// Review B7: the no-directory fallback lives in the driver, not in each
/// recipe step. An empty `--dir` runs the command in the driver's place, with
/// its own output and exit status; a step that already failed exits 1.
#[test]
fn an_empty_dir_runs_the_command_unwrapped_and_reaches_no_agent() {
    let f = Fixture::new();
    let run = |extra: &[&str]| {
        f.amplihack()
            .args(["orch", "helper", "step-recovery", "run", "--step-id", "s1"])
            .args(["--dir", "", "--goal", "g"])
            .args(extra)
            .args([
                "--",
                "sh",
                "-c",
                "echo plain-out; echo plain-err >&2; exit 3",
            ])
            .output()
            .unwrap()
    };
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(3), "{}", show(&o));
    assert_eq!(text(&o.stdout), "plain-out\n");
    assert!(text(&o.stderr).contains("plain-err"), "{}", show(&o));
    let o = run(&["--already-failed", "--essential"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        text(&o.stderr).contains("s1 failed and this run has no step_recovery_dir"),
        "{}",
        show(&o)
    );
    assert!(f.agent_calls().is_empty());
    assert!(f.dir_entries().is_empty(), "{:?}", f.dir_entries());
}
