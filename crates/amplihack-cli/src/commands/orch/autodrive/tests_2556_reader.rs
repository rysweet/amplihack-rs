//! #2556: the evidence reader treats a `null` or missing string as unknown.
//!
//! On v1.2.149 `assess` and `round` stopped with
//! `invalid type: null, expected a string at line 1 column 26`. The reader now
//! reads `null` or a missing value in the listed string fields as `""`, and
//! `validation::unknown_fields` names each such value as an
//! `unknown-field:<path>` blocker. Every other field stays strict.

use super::autodrive_contract_tests::{HEAD, authorization, candidate};
use crate::commands::orch::autodrive::{
    records::{CandidateRecord, FinalizationObservation},
    validation::{is_unknown, unknown_fields},
};
use serde::Deserialize;
use serde_json::{Value, json};

const COLUMN_26: &str = "invalid type: null, expected a string at line 1 column 26";

/// A complete candidate with one failed attempt, as the agent writes it.
fn with_failed_attempt() -> Value {
    let auth = authorization("true", "false");
    let mut raw = candidate(&auth);
    raw["readiness"] = json!("incomplete");
    raw["blockers"] = json!(["checks pending"]);
    raw["attempts"] = json!([{
        "cause": "GraphQL refused", "operation": "gh pr view",
        "outcome": "failed", "recovery": null
    }]);
    raw
}

/// The record serialised compactly with sorted keys, as `serde_json::Value`
/// and `jq -S -c` both write it. `attempts` sorts first and `cause` sorts
/// first inside an attempt.
fn compact_sorted(raw: &Value) -> String {
    compact_member(raw)
}

fn compact_member(v: &Value) -> String {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<_> = o.keys().collect();
            keys.sort();
            let members: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", Value::String((*k).clone()), compact_member(&o[*k])))
                .collect();
            format!("{{{}}}", members.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(compact_member).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

// Probes with the pre-fix field type (`String`). They ignore other keys, so
// serde_json reports the null at exactly the position the old reader did.
#[derive(Deserialize, Debug)]
struct ProbeAttempts {
    #[allow(dead_code)]
    attempts: Vec<ProbeAttempt>,
}
#[derive(Deserialize, Debug)]
struct ProbeAttempt {
    #[allow(dead_code)]
    cause: String,
}
#[derive(Deserialize, Debug)]
struct ProbeBinding {
    #[allow(dead_code)]
    binding: ProbeRunId,
}
#[derive(Deserialize, Debug)]
struct ProbeRunId {
    #[allow(dead_code)]
    run_id: String,
}
#[derive(Deserialize, Debug)]
struct ProbeReviewed {
    #[allow(dead_code)]
    reviewed_head_sha: String,
}

/// Reproduction (D1). Of the three 22-character prefixes, only
/// `{"attempts":[{"cause":` is produced by a compact, sorted writer; the other
/// two need a space after the colon and a hand-ordered first key. The field
/// is `attempts[0].cause`.
#[test]
fn column_26_null_is_attempts_cause() {
    let mut raw = with_failed_attempt();
    raw["attempts"][0]["cause"] = Value::Null;
    let exact = compact_sorted(&raw);
    assert!(
        exact.starts_with(r#"{"attempts":[{"cause":null,"#),
        "{exact}"
    );

    let prefixes = [
        r#"{"attempts":[{"cause":"#,
        r#"{"binding":{"run_id": "#,
        r#"{"reviewed_head_sha": "#,
    ];
    for prefix in prefixes {
        assert_eq!(prefix.len(), 22, "{prefix}");
    }
    // The old String reader gives the exact incident message for each layout.
    let tail = &exact[prefixes[0].len()..];
    let old_attempts = serde_json::from_str::<ProbeAttempts>(&exact).unwrap_err();
    assert_eq!(old_attempts.to_string(), COLUMN_26);
    let old_binding =
        serde_json::from_str::<ProbeBinding>(r#"{"binding":{"run_id": null}}"#).unwrap_err();
    assert_eq!(old_binding.to_string(), COLUMN_26);
    let old_reviewed =
        serde_json::from_str::<ProbeReviewed>(r#"{"reviewed_head_sha": null}"#).unwrap_err();
    assert_eq!(old_reviewed.to_string(), COLUMN_26);
    assert!(tail.starts_with("null,"), "{tail}");

    // The fixed reader parses the exact input and names the field.
    let record: CandidateRecord = serde_json::from_str(&exact)
        .unwrap_or_else(|e| panic!("exact #2556 input must parse, got: {e}"));
    assert_eq!(record.attempts[0].cause, "");
    assert_eq!(
        unknown_fields(&record),
        vec!["unknown-field:attempts[0].cause".to_string()]
    );
}

#[test]
fn missing_cause_reads_like_null_cause() {
    let mut raw = with_failed_attempt();
    raw["attempts"][0].as_object_mut().unwrap().remove("cause");
    let record: CandidateRecord = serde_json::from_value(raw).expect("missing cause parses");
    assert_eq!(record.attempts[0].cause, "");
    assert_eq!(
        unknown_fields(&record),
        vec!["unknown-field:attempts[0].cause".to_string()]
    );
}

#[test]
fn null_in_other_rival_fields_is_tolerated_and_named() {
    let mut raw = with_failed_attempt();
    raw["binding"]["run_id"] = Value::Null;
    raw["reviewed_head_sha"] = Value::Null;
    let record: CandidateRecord = serde_json::from_value(raw).expect("tolerant fields parse");
    let blockers = unknown_fields(&record);
    for expected in [
        "unknown-field:binding.run_id",
        "unknown-field:reviewed_head_sha",
    ] {
        assert!(blockers.iter().any(|b| b == expected), "{blockers:?}");
    }
}

#[test]
fn literal_unknown_and_whitespace_are_unknown() {
    assert!(is_unknown(""));
    assert!(is_unknown("   "));
    assert!(is_unknown("unknown"));
    assert!(!is_unknown(HEAD));
    assert!(!is_unknown("none"));

    let mut raw = with_failed_attempt();
    raw["binding"]["head_sha"] = json!("unknown");
    let record: CandidateRecord = serde_json::from_value(raw).unwrap();
    assert!(
        unknown_fields(&record)
            .iter()
            .any(|b| b == "unknown-field:binding.head_sha")
    );

    for value in [Value::Null, json!("unknown"), json!("  ")] {
        let mut raw = with_failed_attempt();
        raw["binding"]["base_branch"] = value.clone();
        let record: CandidateRecord = serde_json::from_value(raw).unwrap();
        assert!(
            unknown_fields(&record)
                .iter()
                .any(|b| b == "unknown-field:binding.base_branch"),
            "base_branch {value}"
        );
    }
}

#[test]
fn succeeded_attempt_without_cause_is_not_a_blocker() {
    for cause in [Value::Null, json!(""), json!("none")] {
        let mut raw = with_failed_attempt();
        raw["attempts"][0]["outcome"] = json!("succeeded");
        raw["attempts"][0]["cause"] = cause.clone();
        let record: CandidateRecord = serde_json::from_value(raw).unwrap();
        assert!(unknown_fields(&record).is_empty(), "cause {cause}");
    }
}

#[test]
fn nested_tolerant_fields_are_named_by_position() {
    let mut raw = with_failed_attempt();
    raw["attempts"][0]["recovery"] = json!({
        "interface": null, "complete": false, "evidence": "REST read back"
    });
    raw["review_range"] = json!({
        "reviewed_head_sha": HEAD, "verified_head_sha": HEAD,
        "base_sha": HEAD, "observation": null
    });
    raw["criteria"][3]["evidence"][0]["observation"] = Value::Null;
    let record: CandidateRecord = serde_json::from_value(raw).unwrap();
    let blockers = unknown_fields(&record);
    assert!(
        blockers.contains(&"unknown-field:attempts[0].recovery.interface".to_string()),
        "{blockers:?}"
    );
    assert!(
        blockers.contains(&"unknown-field:review_range.observation".to_string()),
        "{blockers:?}"
    );
    assert!(
        blockers
            .iter()
            .any(|b| b.starts_with("unknown-field:criteria[")
                && b.ends_with(".evidence[0].observation")),
        "{blockers:?}"
    );
    let mut sorted = blockers.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(blockers, sorted, "blockers are sorted and unique");
}

#[test]
fn criterion_path_never_echoes_an_id_outside_the_fixed_list() {
    let mut raw = with_failed_attempt();
    raw["criteria"][0]["id"] = json!("$(touch pwned)\u{1b}[31m");
    raw["criteria"][0]["binding"]["head_sha"] = json!("unknown");
    let record: CandidateRecord = serde_json::from_value(raw).unwrap();
    let blockers = unknown_fields(&record);
    assert!(
        blockers
            .iter()
            .any(|b| b.starts_with("unknown-field:criteria[") && b.ends_with("binding.head_sha")),
        "{blockers:?}"
    );
    for b in &blockers {
        assert!(
            !b.contains("pwned") && !b.chars().any(char::is_control),
            "{b:?}"
        );
    }
}

type Mutation = (&'static str, fn(&mut Value));

#[test]
fn strict_fields_and_unknown_keys_remain_parse_errors() {
    let base = with_failed_attempt();
    let mutations: [Mutation; 8] = [
        ("extra key", |r| r["surprise"] = json!("x")),
        ("extra attempt key", |r| {
            r["attempts"][0]["extra"] = json!(1)
        }),
        ("null schema_version", |r| r["schema_version"] = Value::Null),
        ("null readiness", |r| r["readiness"] = Value::Null),
        ("null criterion id", |r| {
            r["criteria"][0]["id"] = Value::Null
        }),
        ("null blocker entry", |r| r["blockers"] = json!([null])),
        ("null outcome", |r| {
            r["attempts"][0]["outcome"] = Value::Null
        }),
        ("null reference file", |r| {
            r["criteria"][0]["evidence"][0]["reference"] = json!({"file": null, "sha256": "ab"})
        }),
    ];
    for (label, mutate) in mutations {
        let mut raw = base.clone();
        mutate(&mut raw);
        assert!(
            serde_json::from_value::<CandidateRecord>(raw).is_err(),
            "{label} must stay a parse error"
        );
    }
}

#[test]
fn serialisation_is_unchanged_and_never_writes_null() {
    let mut raw = with_failed_attempt();
    raw["attempts"][0]["outcome"] = json!("succeeded");
    raw["attempts"][0]["cause"] = json!("");
    let text = serde_json::to_string(&raw).unwrap();
    let record: CandidateRecord = serde_json::from_str(&text).unwrap();
    let once = serde_json::to_vec(&record).unwrap();
    let again =
        serde_json::to_vec(&serde_json::from_slice::<CandidateRecord>(&once).unwrap()).unwrap();
    assert_eq!(once, again, "round trip is byte for byte");
    let written: Value = serde_json::from_slice(&once).unwrap();
    assert_eq!(written["attempts"][0]["cause"], "");

    let mut nulled = with_failed_attempt();
    nulled["attempts"][0]["cause"] = Value::Null;
    let record: CandidateRecord = serde_json::from_value(nulled).unwrap();
    let written: Value = serde_json::to_value(&record).unwrap();
    assert_eq!(
        written["attempts"][0]["cause"], "",
        "null is never written back"
    );
}

#[test]
fn finalization_observation_strings_tolerate_null() {
    let auth = authorization("true", "false");
    let mut raw = json!({
        "binding": super::autodrive_contract_tests::binding(&auth),
        "requested_action": "verify_readiness",
        "expected_head_sha": HEAD, "refreshed_head_sha": HEAD,
        "request_expected_head_sha": null,
        "mutable_facts_refreshed": true, "merge_requests": 0,
        "request_outcome": "not_requested", "platform_state_at_entry": "open",
        "platform_state_before_action": "open", "platform_state": "open",
        "confirmation_evidence": null, "failure_cause": null, "attempts": []
    });
    let observed: FinalizationObservation =
        serde_json::from_value(raw.clone()).expect("null confirmation_evidence parses");
    assert_eq!(observed.confirmation_evidence, "");
    raw["platform_state"] = Value::Null;
    assert!(
        serde_json::from_value::<FinalizationObservation>(raw).is_err(),
        "enum fields stay strict"
    );
}
