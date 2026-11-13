//! Issue #2539: a wrapped step's failure goes to an agent, and every run ends
//! in a recorded choice, never in an unexplained exit.
//!
//! The four failures from the issue, simulated with fake tools: a refused
//! bundle copy, an unset variable in a tool, a head read that GraphQL refuses,
//! and `no_scoped_pr` while the pull request exists. The paths that never
//! reach an agent are in `issue_2539_step_recovery_policy.rs`.

#![cfg(unix)]

use std::fs;

use serde_json::json;

use crate::support::step_recovery::{Fixture, HANDOFF_EXIT, outcome, show, text};

#[test]
fn a_refused_bundle_copy_ends_in_a_recorded_agent_handoff() {
    let f = Fixture::new();
    let guard = f.tool(
        "stage-bundle",
        "#!/bin/sh\necho 'refusing bundle copy: unsafe ownership or modes on staged directory' >&2\nexit 1\n",
    );
    f.choose(
        1,
        json!({"choice": "handoff",
        "reason": "bundle copy refused: unsafe ownership or modes on the staged directory",
        "owner": "run-requester",
        "needs": "fix ownership of the staged bundle directory, then rerun the drive"}),
    );

    let o = f.run("step-02-crusty-loop", &[], &[guard.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "a handoff exits 0:\n{}", show(&o));
    let line = outcome(&o);
    assert_eq!(line["choice"], "handoff");
    assert_eq!(line["owner"], "run-requester");
    assert_eq!(line["step_id"], "step-02-crusty-loop");
    let record_path = line["record"].as_str().expect("outcome names its record");
    assert_eq!(
        record_path,
        f.dir.join("step-02-crusty-loop.jsonl").to_str().unwrap()
    );

    let records = f.records("step-02-crusty-loop");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["source"], "agent");
    assert_eq!(records[0]["failure"]["exit_code"], 1);
    assert!(
        records[0]["failure"]["output_excerpt"]
            .as_str()
            .unwrap()
            .contains("unsafe ownership"),
        "the excerpt holds the guard's refusal: {}",
        records[0]
    );

    let calls = f.agent_calls();
    assert_eq!(calls.len(), 1, "one agent: {calls:?}");
    let fields: Vec<&str> = calls[0].split('\t').collect();
    assert_eq!(
        fields[1], "1",
        "the agent runs with AMPLIHACK_STEP_RECOVERY_ACTIVE=1"
    );
    assert!(fields[2].contains("step-failure-recovery"), "{calls:?}");

    let evidence = f.evidence(1);
    assert_eq!(evidence["step_id"], "step-02-crusty-loop");
    assert_eq!(evidence["goal"], "Merge the fixture pull request");
    assert_eq!(evidence["default_owner"], "run-requester");
    assert!(
        evidence["untrusted_notice"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(evidence["guard"], false);
    assert!(
        evidence.get("state_excerpt").is_none(),
        "--state-file was removed"
    );

    let report = f.report();
    assert_eq!(
        report.status.code(),
        Some(HANDOFF_EXIT),
        "{}",
        show(&report)
    );
    let listed: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(
        listed["step_recovery_handoffs"][0]["step_id"],
        "step-02-crusty-loop"
    );
    assert_eq!(listed["step_recovery_handoffs"][0]["record"], record_path);
}

#[test]
fn an_unset_variable_is_repaired_and_the_retry_succeeds() {
    let f = Fixture::new();
    let tool = f.tool("find-top", "#!/bin/bash\nset -u\necho \"top is $TOP\"\n");
    // The agent's repair: the tool now sets TOP before reading it.
    f.repair(
        1,
        &format!(
            "#!/bin/sh\nt=\"{0}.new\"\nprintf '#!/bin/bash\\nset -u\\nTOP=/fixture/top\\necho \"top is $TOP\"\\n' > \"$t\"\nchmod 755 \"$t\"\nmv -f \"$t\" \"{0}\"\n",
            tool.display()
        ),
    );
    f.choose(
        1,
        json!({"choice": "retry_after_repair",
        "reason": "tool read $TOP under set -u; TOP is now set before first use",
        "repair": "set TOP before first use in the tool script"}),
    );

    let o = f.run("step-03-finalize", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let out = text(&o.stdout);
    assert!(
        out.contains("top is /fixture/top"),
        "the retry's output passes through:\n{out}"
    );
    assert!(
        !out.contains("step_recovery"),
        "a successful retry prints no outcome line:\n{out}"
    );

    let records = f.records("step-03-finalize");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["choice"], "retry_after_repair");
    assert_eq!(records[0]["recurrence"], false);
    assert!(
        records[0]["failure"]["output_excerpt"]
            .as_str()
            .unwrap()
            .contains("TOP")
    );
    assert_eq!(
        f.report().status.code(),
        Some(0),
        "no handoff, report is clean"
    );
}

#[test]
fn a_refused_graphql_head_read_runs_the_declared_rest_alternative() {
    let f = Fixture::new();
    f.tool(
        "gh",
        "#!/bin/sh\ncase \"$2\" in\n  graphql) echo 'GraphQL: was submitted too quickly (secondary rate limit)' >&2; exit 1 ;;\n  repos/*) echo 0123456789abcdef0123456789abcdef01234567 ;;\nesac\n",
    );
    f.choose(
        1,
        json!({"choice": "alternative_step",
        "reason": "GraphQL refused the head read; the REST read gives the same head",
        "alternative": "rest-head-read"}),
    );

    let alt = "rest-head-read=gh api repos/fixture/repo/pulls/7 --jq .head.sha";
    let o = f.run(
        "step-02-merge-ready-loop",
        &["--alternative", alt],
        &["gh", "api", "graphql", "-f", "query=headRefOid"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        text(&o.stdout).contains("0123456789abcdef0123456789abcdef01234567"),
        "{}",
        show(&o)
    );

    let records = f.records("step-02-merge-ready-loop");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["choice"], "alternative_step");
    assert_eq!(records[0]["alternative"], "rest-head-read");

    let evidence = f.evidence(1);
    assert_eq!(evidence["alternatives"], json!(["rest-head-read"]));
    let raw = fs::read_to_string(f.agent.join("evidence-1.json")).unwrap();
    assert!(
        !raw.contains("repos/fixture/repo"),
        "the agent sees names only, never commands"
    );
}

#[test]
fn no_scoped_pr_while_the_pr_exists_continues_with_the_item_recorded() {
    let f = Fixture::new();
    let ready = f.tool(
        "pr-ready",
        "#!/bin/sh\necho 'no_scoped_pr: no pull request found for this branch'\nexit 1\n",
    );
    f.choose(1, json!({"choice": "continue_recorded",
        "reason": "the branch PR exists and is already ready for review; the scope lookup missed it"}));

    let o = f.run("step-21-pr-ready", &[], &[ready.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let line = outcome(&o);
    assert_eq!(line["choice"], "continue_recorded");
    assert!(
        line["record"]
            .as_str()
            .is_some_and(|p| p.ends_with("step-21-pr-ready.jsonl"))
    );

    let records = f.records("step-21-pr-ready");
    assert_eq!(records[0]["choice"], "continue_recorded");
    assert_eq!(records[0]["source"], "agent");
    assert_eq!(
        f.report().status.code(),
        Some(0),
        "continue_recorded is not a handoff"
    );
}

#[test]
fn no_scoped_pr_may_instead_hand_off_to_the_callers_owner() {
    let f = Fixture::new();
    let ready = f.tool("pr-ready", "#!/bin/sh\necho 'no_scoped_pr' >&2\nexit 1\n");
    f.choose(
        1,
        json!({"choice": "handoff", "reason": "the ready mutation is refused",
        "owner": "pr-author", "needs": "mark the pull request ready by hand"}),
    );

    let o = f.run(
        "step-21-pr-ready",
        &["--owner", "pr-author"],
        &[ready.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(outcome(&o)["owner"], "pr-author");
    let records = f.records("step-21-pr-ready");
    assert_eq!(
        (records[0]["owner"].as_str(), records[0]["source"].as_str()),
        (Some("pr-author"), Some("agent"))
    );
    assert_eq!(f.report().status.code(), Some(HANDOFF_EXIT));
}
