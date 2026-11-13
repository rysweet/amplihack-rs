//! Unit tests for the pure parts of `step_recovery` (issue #2539). Wired from
//! `mod.rs` with `#[cfg(test)] mod tests;`. Behaviour through the CLI is in
//! `crates/amplihack-cli/tests/issue_2539_step_recovery_*.rs`.
//!
//! Contract assumed here (implementation order steps 1-3 of the design):
//! - `STEP_RECOVERY_HANDOFF_EXIT: i32 = 81`, `SCHEMA_VERSION: u32 = 1`;
//! - `signature::signature(exit_code: Option<i32>, class: Option<&str>,
//!   redacted_tail: &str) -> String`;
//! - `record::validate_caller_step_id(&str) -> anyhow::Result<()>` and
//!   `record::sanitise_step_id(&str) -> String` (`:alt:` → `__alt__`);
//! - `record::Record` (serde, `deny_unknown_fields`) with
//!   `Record::check(&self) -> anyhow::Result<()>` for the field invariants;
//! - `evidence::redact(&str) -> String`.

use super::evidence::redact;
use super::record::{Record, sanitise_step_id, validate_caller_step_id};
use super::signature::signature;
use super::{SCHEMA_VERSION, STEP_RECOVERY_HANDOFF_EXIT};

// ── Exit code and schema ────────────────────────────────────────────────────

#[test]
fn the_handoff_exit_is_81_and_clashes_with_no_other_amplihack_exit() {
    assert_eq!(STEP_RECOVERY_HANDOFF_EXIT, 81);
    let taken = (70..=77).chain([79, 80, 86, 91]).chain(97..=99);
    for code in taken {
        assert_ne!(STEP_RECOVERY_HANDOFF_EXIT, code, "{code} is already used");
    }
    assert_eq!(SCHEMA_VERSION, 1);
}

// ── Signature ───────────────────────────────────────────────────────────────

fn tail_of(sig: &str) -> &str {
    sig.rsplit_once("tail=").expect("tail= part").1
}

#[test]
fn signature_has_the_documented_shape() {
    let sig = signature(Some(1), None, "error: index.lock exists");
    assert!(sig.starts_with("exit=1;tail="), "{sig}");
    let tail = tail_of(&sig);
    assert_eq!(tail.len(), 16);
    assert!(
        tail.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(signature(None, None, "x").starts_with("exit=none;tail="));
    let classed = signature(Some(1), Some("agent_api"), "x");
    assert!(
        classed.starts_with("exit=1;class=agent_api;tail="),
        "{classed}"
    );
}

#[test]
fn ids_pids_and_timestamps_do_not_change_the_signature() {
    let a = signature(
        Some(1),
        None,
        "lock held by pid 4242 at 2026-11-12T21:04:33Z ref 9d521815af01",
    );
    let b = signature(
        Some(1),
        None,
        "lock held by pid 77 at 2026-11-13T01:00:00Z ref 0badc0ffee99",
    );
    assert_eq!(a, b);
}

#[test]
fn whitespace_and_blank_lines_do_not_change_the_signature() {
    let a = signature(Some(2), None, "first\n\n   second   \n");
    let b = signature(Some(2), None, "  first\nsecond");
    assert_eq!(a, b);
}

#[test]
fn a_different_message_or_exit_code_changes_the_signature() {
    let base = signature(Some(1), None, "error: index.lock exists");
    assert_ne!(base, signature(Some(1), None, "error: remote rejected"));
    assert_ne!(base, signature(Some(2), None, "error: index.lock exists"));
    assert_ne!(base, signature(None, None, "error: index.lock exists"));
}

#[test]
fn short_hex_runs_are_digit_runs_not_ids() {
    // Runs of 7+ hex digits become H; shorter ones keep their letters.
    let a = signature(Some(1), None, "code beef failed");
    let b = signature(Some(1), None, "code cafe failed");
    assert_ne!(a, b);
}

// ── Step ids ────────────────────────────────────────────────────────────────

#[test]
fn caller_step_ids_follow_the_documented_pattern() {
    let (max, over) = ("x".repeat(128), "x".repeat(129));
    for ok in ["step-21-pr-ready", "a", "A.b_c-9", max.as_str()] {
        assert!(validate_caller_step_id(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        ".",
        "..",
        "a:b",
        "s:alt:x",
        "a/b",
        "a b",
        "a\nb",
        over.as_str(),
        "é",
    ] {
        assert!(validate_caller_step_id(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn sanitising_maps_alt_ids_and_never_shortens() {
    assert_eq!(sanitise_step_id("step-21-pr-ready"), "step-21-pr-ready");
    assert_eq!(
        sanitise_step_id("step-02:alt:rest-read"),
        "step-02__alt__rest-read"
    );
    let long = format!("{}:alt:{}", "s".repeat(128), "a".repeat(64));
    // `:alt:` (5 bytes) becomes `__alt__` (7 bytes); nothing is cut.
    assert_eq!(sanitise_step_id(&long).len(), long.len() + 2);
}

// ── Redaction ───────────────────────────────────────────────────────────────

#[test]
fn redaction_removes_every_documented_secret_form() {
    let secrets = [
        format!("ghp_{}", "A1b2C3d4E5".repeat(4)),
        format!("gho_{}", "A1b2C3d4E5".repeat(4)),
        format!("ghs_{}", "A1b2C3d4E5".repeat(4)),
        format!("ghu_{}", "A1b2C3d4E5".repeat(4)),
        format!("github_pat_{}", "11ABCDEFG0".repeat(5)),
        format!("sk-ant-api03-{}", "Zz9Yy8Xx7W".repeat(4)),
    ];
    for secret in &secrets {
        let out = redact(&format!("push failed: {secret} rejected"));
        assert!(!out.contains(secret.as_str()), "{out}");
        assert!(
            out.contains("push failed:") && out.contains("rejected"),
            "{out}"
        );
    }
    let pairs = [
        (
            "Authorization: Bearer eyJhbGc.payload.sig",
            "eyJhbGc.payload.sig",
        ),
        ("authorization: token abcdef123456", "abcdef123456"),
        ("token=tok-value-1", "tok-value-1"),
        ("password=hunter2", "hunter2"),
        ("secret=s3cr3t", "s3cr3t"),
        ("api_key=k-123", "k-123"),
        ("https://bot:pw-in-url@example.invalid/r.git", "pw-in-url"),
    ];
    for (line, secret) in pairs {
        assert!(!redact(line).contains(secret), "{line} -> {}", redact(line));
    }
}

#[test]
fn redaction_leaves_ordinary_output_alone() {
    let text = "error: index.lock exists\nhint: remove it and retry";
    assert_eq!(redact(text), text);
}

// ── Records ─────────────────────────────────────────────────────────────────

fn parse(json: serde_json::Value) -> anyhow::Result<Record> {
    let record: Record = serde_json::from_value(json)?;
    record.check()?;
    Ok(record)
}

fn base(choice: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "step_id": "step-21-pr-ready",
        "failure": {"exit_code": 1, "signature": "exit=1;tail=3f9a0c51d2e87b46", "output_excerpt": "no_scoped_pr"},
        "choice": choice,
        "reason": "the PR exists",
        "recurrence": false,
        "source": "agent",
        "recorded_at": "2026-11-12T21:04:33Z"
    })
}

#[test]
fn a_documented_record_parses() {
    assert!(parse(base("continue_recorded")).is_ok());
    let mut handoff = base("handoff");
    handoff["owner"] = "run-requester".into();
    handoff["needs"] = "mark it ready".into();
    assert!(parse(handoff).is_ok());
}

#[test]
fn records_refuse_unknown_fields_and_mismatched_fields() {
    let mut extra = base("continue_recorded");
    extra["confidence"] = 0.9.into();
    assert!(parse(extra).is_err(), "unknown field");

    let mut owner_without_handoff = base("continue_recorded");
    owner_without_handoff["owner"] = "run-requester".into();
    assert!(parse(owner_without_handoff).is_err());

    assert!(
        parse(base("handoff")).is_err(),
        "handoff needs owner and needs"
    );
    assert!(
        parse(base("retry_after_repair")).is_err(),
        "retry needs repair"
    );
    assert!(
        parse(base("alternative_step")).is_err(),
        "alternative needs a name"
    );

    let mut bad_source = base("continue_recorded");
    bad_source["source"] = "operator".into();
    assert!(parse(bad_source).is_err());

    let mut v2 = base("continue_recorded");
    v2["schema_version"] = 2.into();
    assert!(parse(v2).is_err());
}

#[test]
fn exactly_one_field_explains_the_failure_status() {
    let mut both = base("continue_recorded");
    both["failure"]["unavailable_reason"] = "runner gap".into();
    assert!(
        parse(both).is_err(),
        "exit_code and unavailable_reason together"
    );

    let mut none = base("continue_recorded");
    none["failure"]["exit_code"] = serde_json::Value::Null;
    assert!(
        parse(none).is_err(),
        "null exit_code with nothing to explain it"
    );

    let mut signal = base("continue_recorded");
    signal["failure"]["exit_code"] = serde_json::Value::Null;
    signal["failure"]["signal"] = 15.into();
    assert!(parse(signal).is_ok());
}

#[test]
fn record_text_fields_refuse_control_characters_and_overlong_reasons() {
    let mut esc = base("continue_recorded");
    esc["reason"] = "ok\u{1b}[2J".into();
    assert!(parse(esc).is_err());
    let mut long = base("continue_recorded");
    long["reason"] = "x".repeat(1025).into();
    assert!(parse(long).is_err());
    let mut empty = base("continue_recorded");
    empty["reason"] = "".into();
    assert!(parse(empty).is_err());
}
