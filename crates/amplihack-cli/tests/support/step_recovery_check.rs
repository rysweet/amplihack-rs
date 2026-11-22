//! Issue #2539: checks on `step-recovery` outcome lines and records, against
//! the format in `docs/reference/step-failure-recovery.md`.

#![allow(dead_code)]

use std::process::Output;

use serde_json::Value;

use crate::support::step_recovery::{show, text};

/// The `{"step_recovery":…}` outcome line, which must be the last stdout line.
pub fn outcome(o: &Output) -> Value {
    let out = text(&o.stdout);
    let last = out.lines().rev().find(|l| !l.trim().is_empty());
    let value: Value = last
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| panic!("no outcome line:\n{}", show(o)));
    let inner = value["step_recovery"].clone();
    assert!(
        inner.is_object(),
        "outcome line lacks step_recovery:\n{}",
        show(o)
    );
    assert!(
        inner.get("output_excerpt").is_none(),
        "the outcome line must never print output_excerpt"
    );
    inner
}

const CHOICES: [&str; 4] = [
    "retry_after_repair",
    "alternative_step",
    "continue_recorded",
    "handoff",
];

/// Checks one record against the documented format and its invariants.
pub fn assert_valid_record(r: &Value) {
    let ctx = || r.to_string();
    assert_eq!(r["schema_version"], 1, "{}", ctx());
    assert!(
        r["step_id"].as_str().is_some_and(|s| !s.is_empty()),
        "{}",
        ctx()
    );
    let choice = r["choice"].as_str().unwrap_or_default();
    assert!(CHOICES.contains(&choice), "bad choice: {}", ctx());
    let reason = r["reason"].as_str().unwrap_or_default();
    assert!(
        (1..=1024).contains(&reason.len()),
        "reason length: {}",
        ctx()
    );
    assert!(
        !reason.chars().any(char::is_control),
        "control chars: {}",
        ctx()
    );
    assert!(r["recurrence"].is_boolean(), "{}", ctx());
    let source = r["source"].as_str().unwrap_or_default();
    assert!(
        ["agent", "validator", "policy"].contains(&source),
        "{}",
        ctx()
    );
    assert!(
        r["recorded_at"].as_str().is_some_and(|t| t.ends_with('Z')),
        "{}",
        ctx()
    );
    let handoff = choice == "handoff";
    for key in ["owner", "needs"] {
        assert_eq!(
            r.get(key).is_some(),
            handoff,
            "{key} only with handoff: {}",
            ctx()
        );
    }
    assert_eq!(
        r.get("repair").is_some(),
        choice == "retry_after_repair",
        "{}",
        ctx()
    );
    assert_eq!(
        r.get("alternative").is_some(),
        choice == "alternative_step",
        "{}",
        ctx()
    );
    let failure = &r["failure"];
    let explained = [
        failure["exit_code"].is_i64(),
        failure.get("signal").is_some(),
        failure.get("unavailable_reason").is_some(),
    ];
    assert_eq!(
        explained.iter().filter(|b| **b).count(),
        1,
        "status: {}",
        ctx()
    );
    assert_signature(failure["signature"].as_str().unwrap_or_default());
    assert!(failure["output_excerpt"].is_string(), "{}", ctx());
}

/// `exit=<n|none>;[class=<c>;]tail=<16 hex>`.
pub fn assert_signature(sig: &str) {
    let rest = sig
        .strip_prefix("exit=")
        .unwrap_or_else(|| panic!("signature {sig}"));
    let (exit, rest) = rest
        .split_once(';')
        .unwrap_or_else(|| panic!("signature {sig}"));
    assert!(
        exit == "none" || exit.parse::<i32>().is_ok(),
        "signature {sig}"
    );
    let rest = match rest.strip_prefix("class=") {
        Some(r) => {
            r.split_once(';')
                .unwrap_or_else(|| panic!("signature {sig}"))
                .1
        }
        None => rest,
    };
    let tail = rest
        .strip_prefix("tail=")
        .unwrap_or_else(|| panic!("signature {sig}"));
    assert!(
        tail.len() == 16
            && tail
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "signature {sig}"
    );
}
