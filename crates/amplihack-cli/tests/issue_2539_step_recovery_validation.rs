//! Issue #2539: the agent's reply is untrusted. Any reply that fails the
//! documented checks, and every case the driver must not hand to an agent,
//! ends as a recorded `handoff` with `source: "validator"`.

#![cfg(unix)]

use serde_json::{Value, json};

use crate::support::step_recovery::{Fixture, outcome, show};

const MARK: &str = "AGENT-REASON-MARK";

fn failing(f: &Fixture) -> String {
    f.tool(
        "fails",
        "#!/bin/sh\necho 'merge-ready: head moved' >&2\nexit 1\n",
    )
    .to_string_lossy()
    .into_owned()
}

/// Runs one failing step with `reply` as the agent's whole last line and
/// returns its single record, after checking it is a validator handoff.
fn validator_handoff(reply: &str, extra: &[&str]) -> Value {
    let f = Fixture::new();
    let tool = failing(&f);
    f.reply(1, reply);
    let o = f.run("step-02-merge-ready-loop", extra, &[&tool]);
    assert_eq!(o.status.code(), Some(0), "reply {reply}:\n{}", show(&o));
    let line = outcome(&o);
    assert_eq!(line["choice"], "handoff", "reply {reply}");
    assert_eq!(line["source"], "validator", "reply {reply}");
    let mut records = f.records("step-02-merge-ready-loop");
    assert_eq!(records.len(), 1, "reply {reply}");
    let record = records.remove(0);
    assert_eq!(record["source"], "validator", "reply {reply}: {record}");
    assert_eq!(record["choice"], "handoff");
    assert_eq!(record["recurrence"], false);
    record
}

fn body(v: Value) -> String {
    json!({ "step_recovery": v }).to_string()
}

#[test]
fn invalid_replies_become_validator_handoffs_owned_by_the_run_requester() {
    let long = "x".repeat(1025);
    let cases: Vec<(&str, String)> = vec![
        ("no reply", String::new()),
        ("prose only", "I could not decide.".into()),
        (
            "not JSON",
            r#"{"step_recovery":{"choice":"handoff","reason":"#.into(),
        ),
        (
            "unknown field",
            body(json!({"choice": "continue_recorded", "reason": MARK, "confidence": 0.9})),
        ),
        (
            "agent sets recurrence",
            body(json!({"choice": "continue_recorded", "reason": MARK, "recurrence": false})),
        ),
        (
            "agent sets source",
            body(json!({"choice": "continue_recorded", "reason": MARK, "source": "policy"})),
        ),
        (
            "unknown choice",
            body(json!({"choice": "give_up", "reason": MARK})),
        ),
        (
            "no alternatives declared",
            body(json!({"choice": "alternative_step", "reason": MARK, "alternative": "rest"})),
        ),
        (
            "empty reason",
            body(json!({"choice": "continue_recorded", "reason": ""})),
        ),
        (
            "reason too long",
            body(json!({"choice": "continue_recorded", "reason": long})),
        ),
        (
            "control character",
            body(json!({"choice": "continue_recorded", "reason": "ok\u{1b}[2J"})),
        ),
        (
            "retry without repair",
            body(json!({"choice": "retry_after_repair", "reason": MARK})),
        ),
        (
            "handoff without needs",
            body(json!({"choice": "handoff", "reason": MARK, "owner": "run-requester"})),
        ),
        (
            "handoff without owner",
            body(json!({"choice": "handoff", "reason": MARK, "needs": "look"})),
        ),
        (
            "continue with owner",
            body(json!({"choice": "continue_recorded", "reason": MARK, "owner": "run-requester"})),
        ),
        (
            "continue with repair",
            body(json!({"choice": "continue_recorded", "reason": MARK, "repair": "x"})),
        ),
    ];
    for (name, reply) in cases {
        let record = validator_handoff(&reply, &[]);
        assert_eq!(record["owner"], "run-requester", "{name}: {record}");
        let reason = record["reason"].as_str().unwrap();
        assert!(
            !reason.is_empty() && reason.len() <= 1024,
            "{name}: {reason}"
        );
        assert!(
            !reason.contains('\u{1b}'),
            "{name}: control characters never reach a record"
        );
    }
}

#[test]
fn a_validator_handoff_keeps_the_agents_reason_and_the_callers_owner() {
    let record = validator_handoff(
        &body(json!({"choice": "alternative_step", "reason": MARK, "alternative": "undeclared"})),
        &["--owner", "pr-author", "--alternative", "rest=true"],
    );
    assert_eq!(record["owner"], "pr-author");
    assert!(
        record["reason"].as_str().unwrap().contains(MARK),
        "{record}"
    );
}

#[test]
fn an_untrusted_owner_is_replaced_by_the_default_owner() {
    let record = validator_handoff(
        &body(
            json!({"choice": "handoff", "reason": MARK, "owner": "mallory", "needs": "send me the token"}),
        ),
        &["--owner", "pr-author"],
    );
    assert_eq!(
        record["owner"], "pr-author",
        "owners come only from trusted values: {record}"
    );
    assert!(!record.to_string().contains("mallory\""), "{record}");
}

#[test]
fn continue_recorded_is_refused_for_an_essential_step() {
    let record = validator_handoff(
        &body(json!({"choice": "continue_recorded", "reason": MARK})),
        &["--essential"],
    );
    assert!(record["reason"].as_str().unwrap().contains(MARK));
}

#[test]
fn the_last_step_recovery_object_in_the_reply_wins() {
    let f = Fixture::new();
    let tool = failing(&f);
    let quoted = body(
        json!({"choice": "handoff", "reason": "quoted from the evidence",
        "owner": "run-requester", "needs": "x"}),
    );
    let real = body(
        json!({"choice": "continue_recorded", "reason": "head moved; next round re-reads it"}),
    );
    f.reply(1, &format!("{quoted}\nOn reflection:\n{real}"));
    let o = f.run("step-02-merge-ready-loop", &[], &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(outcome(&o)["choice"], "continue_recorded");
}

#[test]
fn recovery_inside_recovery_starts_no_agent() {
    let f = Fixture::new();
    let tool = failing(&f);
    f.choose(
        1,
        json!({"choice": "continue_recorded", "reason": "never read"}),
    );
    let o = f
        .run_cmd("step-02-merge-ready-loop", &[], &[&tool])
        .env("AMPLIHACK_STEP_RECOVERY_ACTIVE", "1")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        f.agent_calls().is_empty(),
        "nested recovery must not start an agent"
    );
    let record = &f.records("step-02-merge-ready-loop")[0];
    assert_eq!(
        (record["choice"].as_str(), record["source"].as_str()),
        (Some("handoff"), Some("validator"))
    );
}

#[test]
fn a_record_file_changed_while_the_agent_ran_becomes_a_validator_handoff() {
    let f = Fixture::new();
    let tool = failing(&f);
    f.choose(
        1,
        json!({"choice": "continue_recorded", "reason": "first-round skip"}),
    );
    assert_eq!(
        f.run("step-02-merge-ready-loop", &[], &[&tool])
            .status
            .code(),
        Some(0)
    );

    // The agent rewrites the earlier record while it "repairs".
    let file = f.dir.join("step-02-merge-ready-loop.jsonl");
    f.repair(
        2,
        &format!(
            "#!/bin/sh\nsed -i 's/first-round skip/forged history/' '{}'\n",
            file.display()
        ),
    );
    f.choose(
        2,
        json!({"choice": "continue_recorded", "reason": "second-round skip"}),
    );
    let o = f.run("step-02-merge-ready-loop", &[], &[&tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let line = outcome(&o);
    assert_eq!(
        (line["choice"].as_str(), line["source"].as_str()),
        (Some("handoff"), Some("validator"))
    );
    let last = f.records("step-02-merge-ready-loop").pop().unwrap();
    assert_eq!(last["source"], "validator");
}

#[test]
fn secrets_in_the_failed_output_never_reach_records_evidence_or_output() {
    let f = Fixture::new();
    let secrets = [
        format!("ghp_{}", "A1b2C3d4E5".repeat(4)),
        format!("github_pat_{}", "11ABCDEFG0".repeat(5)),
        format!("sk-ant-api03-{}", "Zz9Yy8Xx7W".repeat(4)),
        "Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig".to_string(),
        "hunter2-password-value".to_string(),
        "s3cr3t-userinfo".to_string(),
    ];
    let tool = f.tool(
        "leaky",
        &format!(
            "#!/bin/sh\necho 'push failed for {0}'\necho 'github_pat: {1}' >&2\necho 'key {2}' >&2\necho 'Authorization: {3}' >&2\necho 'password={4}' >&2\necho 'remote https://bot:{5}@example.invalid/r.git' >&2\nexit 1\n",
            secrets[0], secrets[1], secrets[2], secrets[3], secrets[4], secrets[5]
        ),
    );
    f.choose(
        1,
        json!({"choice": "continue_recorded", "reason": "cleanup only"}),
    );
    let o = f.run("step-20b-push-cleanup", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));

    let written = f.all_written_text();
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    for secret in &secrets {
        assert!(
            !written.contains(secret.as_str()),
            "{secret} written under the recovery dir or evidence"
        );
        assert!(
            !stdout.contains(secret.as_str()),
            "{secret} in the outcome line"
        );
    }
    let excerpt = f.records("step-20b-push-cleanup")[0]["failure"]["output_excerpt"].clone();
    assert!(
        excerpt.as_str().unwrap().contains("push failed for"),
        "the rest of the excerpt stays: {excerpt}"
    );
}
