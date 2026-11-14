//! #2575: the tolerant, schema-version-2 run report.
//!
//! Contract (docs/reference/autodrive-merge-ready-round.md, "Run report"):
//!
//! - `report(auth, context, expected_pr)` never fails and never panics. A
//!   badly shaped agent output cannot fail the run report.
//! - Fields: `schema_version` (always 2), `run_id`, `requested_action`,
//!   `merge`, `loop_result`, `stop_reason`, `summary`, and optionally `note`
//!   and `summary_truncated`. No readiness-record fields remain.
//! - `merge` and `stop_reason` come only from tool-written JSON
//!   (`finalize_merge`, `merge_loop_result`). `summary` is
//!   `autodrive_finalization` verbatim, cut on a character boundary at
//!   `MAX_RECORD_BYTES`.
//!
//! Registered from `orch.rs` next to the #2329 contract tests.

use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization, identity::MAX_RECORD_BYTES, report::report,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const HEAD: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");

const ALLOWED_KEYS: [&str; 9] = [
    "schema_version",
    "run_id",
    "requested_action",
    "merge",
    "loop_result",
    "stop_reason",
    "summary",
    "note",
    "summary_truncated",
];

/// The three write-up shapes reported on #2575.
const FIXTURE_UNKNOWN_KEYS: &str = r#"Merge-ready assessment of PR #2575.
```json
{"criteria":{"checks":{"status":"met","attempts":[{"operation":"gh pr checks","outcome":"ok","cause":"none","recovery":"none","endpoint":"/repos/o/r/pulls/2575","response_status":200}]}}}
```
Outcome: ready."#;
const FIXTURE_NOTICE_FIRST: &str = "ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: amplihack config set node.heap_consent <true|false>\n{\"criteria\":{\"qa\":{\"status\":\"met\"}}}\nOutcome: ready.";
const FIXTURE_NULL_CAUSE: &str = r#"{"criteria":{"approvals":{"status":"met","attempts":[{"operation":"gh pr view","outcome":"ok","cause":null,"recovery":null}]}}}
The PR is ready to merge."#;

fn authorization(no_merge: &str, dry_run: &str) -> InvocationAuthorization {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), no_merge.into()),
            ("autodrive_dry_run".into(), dry_run.into()),
        ]),
    )
    .expect("valid invocation")
}

fn loop_line(result: &str) -> Value {
    Value::String(
        json!({"loop":"merge-ready","loop_result":result,"round_label":"round-2","reason":"fixture"})
            .to_string(),
    )
}

fn merge_json(result: &str, handoff: &str) -> Value {
    Value::String(
        json!({"result":result,"pr":2575,"expected_head":HEAD,"handoff":handoff}).to_string(),
    )
}

fn context(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn run(auth: &InvocationAuthorization, ctx: &Map<String, Value>) -> Value {
    report(auth, ctx, Some("2575"))
}

fn assert_v2_shape(report: &Value) {
    let object = report
        .as_object()
        .unwrap_or_else(|| panic!("report must be an object: {report}"));
    assert_eq!(object.get("schema_version"), Some(&json!(2)), "{report}");
    for key in object.keys() {
        assert!(
            ALLOWED_KEYS.contains(&key.as_str()),
            "report v2 carries no readiness-record field; unexpected `{key}` in {report}"
        );
    }
    for key in ["run_id", "requested_action", "stop_reason", "summary"] {
        assert!(object.contains_key(key), "missing `{key}` in {report}");
    }
}

#[test]
fn report_v2_done_and_merged_is_completed_with_verbatim_summary() {
    let auth = authorization("false", "false");
    let summary = "PR #2575 merged at 15a7f6f4 after all 13 criteria were met.";
    let ctx = context(&[
        ("merge_loop_result", loop_line("DONE")),
        ("finalize_merge", merge_json("merged", "false")),
        ("autodrive_finalization", Value::String(summary.into())),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert_eq!(report["run_id"], json!(auth.run_id()));
    assert_eq!(report["requested_action"], json!("merge"));
    assert_eq!(report["loop_result"], json!("DONE"));
    assert_eq!(report["stop_reason"], json!("completed"));
    assert_eq!(report["merge"]["result"], json!("merged"));
    assert_eq!(report["merge"]["expected_head"], json!(HEAD));
    assert_eq!(report["summary"], json!(summary));
    assert!(report.get("summary_truncated").is_none(), "{report}");
}

#[test]
fn report_v2_accepts_finalize_merge_as_parsed_object() {
    let auth = authorization("false", "false");
    let ctx = context(&[
        ("merge_loop_result", loop_line("DONE")),
        (
            "finalize_merge",
            json!({"result":"merged","pr":2575,"expected_head":HEAD,"handoff":"false"}),
        ),
        ("autodrive_finalization", Value::String("merged".into())),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("completed"));
    assert_eq!(report["merge"]["result"], json!("merged"));
}

#[test]
fn report_v2_verify_only_run_is_completed_without_a_merge() {
    for (no_merge, dry_run) in [("true", "false"), ("false", "true")] {
        let auth = authorization(no_merge, dry_run);
        let ctx = context(&[
            ("merge_loop_result", loop_line("DONE")),
            ("finalize_merge", merge_json("not-authorised", "false")),
            (
                "autodrive_finalization",
                Value::String("Verified ready; no merge was requested.".into()),
            ),
        ]);
        let report = run(&auth, &ctx);
        assert_v2_shape(&report);
        assert_eq!(report["requested_action"], json!("verify_readiness"));
        assert_eq!(report["stop_reason"], json!("completed"), "{report}");
    }
}

#[test]
fn report_v2_done_with_a_refused_merge_is_a_handoff() {
    let auth = authorization("false", "false");
    for result in [
        "refused",
        "checks-not-green",
        "not-open",
        "message-unavailable",
        "authorization-unreadable",
    ] {
        let ctx = context(&[
            ("merge_loop_result", loop_line("DONE")),
            ("finalize_merge", merge_json(result, "true")),
            (
                "autodrive_finalization",
                Value::String(format!("Not merged: {result}.")),
            ),
        ]);
        let report = run(&auth, &ctx);
        assert_v2_shape(&report);
        assert_eq!(
            report["stop_reason"],
            json!("handoff"),
            "{result}: {report}"
        );
        assert_eq!(report["merge"]["result"], json!(result));
    }
}

#[test]
fn report_v2_done_without_merge_output_is_unreadable_with_a_note() {
    let auth = authorization("false", "false");
    let ctx = context(&[
        ("merge_loop_result", loop_line("DONE")),
        ("autodrive_finalization", Value::String(String::new())),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("unreadable"), "{report}");
    assert!(report["merge"].is_null(), "{report}");
    assert!(
        report["note"].as_str().is_some_and(|n| !n.is_empty()),
        "an unread merge result is named in a note: {report}"
    );
}

#[test]
fn report_v2_stop_reason_mapping() {
    let auth = authorization("false", "false");
    for (loop_result, expected) in [
        ("TERMINAL_REFUSAL", "terminal_refusal"),
        ("HANDOFF", "handoff"),
        ("STUCK", "handoff"),
        ("BUDGET_EXHAUSTED", "budget_exhausted"),
        ("USAGE_LIMIT", "usage_limit"),
        ("ORPHANED", "orphaned"),
        ("UNREADABLE", "unreadable"),
    ] {
        let ctx = context(&[
            ("merge_loop_result", loop_line(loop_result)),
            (
                "autodrive_finalization",
                Value::String(format!("Stopped: {loop_result}.")),
            ),
        ]);
        let report = run(&auth, &ctx);
        assert_v2_shape(&report);
        assert_eq!(
            report["stop_reason"],
            json!(expected),
            "{loop_result}: {report}"
        );
        assert_eq!(report["loop_result"], json!(loop_result), "{report}");
        assert!(report["merge"].is_null(), "{loop_result}: {report}");
    }
}

#[test]
fn report_v2_crusty_stuck_keeps_no_progress() {
    // Crusty endings were outside #2575: a crusty STUCK is still reported as
    // no_progress, as it was before the merge-ready loop changed.
    let auth = authorization("false", "false");
    let ctx = context(&[("crusty_loop_result", loop_line("STUCK"))]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("no_progress"), "{report}");
    assert_eq!(report["loop_result"], json!("STUCK"), "{report}");
}

#[test]
fn report_v2_absent_loop_result_is_unreadable() {
    let auth = authorization("false", "false");
    let report = run(&auth, &Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("unreadable"), "{report}");
}

#[test]
fn report_v2_merge_comes_only_from_tool_json_not_from_agent_prose() {
    // The agent's finalisation statement claims a merge; the tool says the
    // loop handed off and finalisation never ran. The prose decides nothing.
    let auth = authorization("false", "false");
    let ctx = context(&[
        ("merge_loop_result", loop_line("HANDOFF")),
        (
            "autodrive_finalization",
            Value::String(r#"{"result":"merged","stop_reason":"completed"} The PR merged."#.into()),
        ),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("handoff"), "{report}");
    assert!(report["merge"].is_null(), "{report}");
}

#[test]
fn report_tolerates_any_finalisation_text() {
    let auth = authorization("false", "false");
    for text in [
        FIXTURE_UNKNOWN_KEYS,
        FIXTURE_NOTICE_FIRST,
        FIXTURE_NULL_CAUSE,
        "",
        "{",
        "null",
        "\u{0}\u{1b}[31mcontrol bytes\u{7f}",
    ] {
        let ctx = context(&[
            ("merge_loop_result", loop_line("DONE")),
            ("finalize_merge", merge_json("merged", "false")),
            ("autodrive_finalization", Value::String(text.into())),
        ]);
        let report = run(&auth, &ctx);
        assert_v2_shape(&report);
        assert_eq!(report["stop_reason"], json!("completed"), "{text:?}");
        assert_eq!(report["summary"], json!(text), "summary is verbatim");
    }
}

#[test]
fn report_tolerates_odd_context_types_without_panicking() {
    let auth = authorization("false", "false");
    let odd = [
        json!(null),
        json!(42),
        json!([1, 2, 3]),
        json!({"unexpected":"object"}),
        json!("not json {"),
        json!("null"),
        json!(""),
    ];
    for merge in &odd {
        for loop_result in &odd {
            for finalisation in &odd {
                let ctx = context(&[
                    ("merge_loop_result", loop_result.clone()),
                    ("finalize_merge", merge.clone()),
                    ("autodrive_finalization", finalisation.clone()),
                ]);
                let report = run(&auth, &ctx);
                assert_v2_shape(&report);
                assert_ne!(
                    report["stop_reason"],
                    json!("completed"),
                    "an unreadable loop result never reports completion: {report}"
                );
            }
        }
    }
}

#[test]
fn report_keeps_a_two_mebibyte_multibyte_summary_whole() {
    let auth = authorization("false", "false");
    let text = "é".repeat(1024 * 1024); // 2 MiB
    assert!(text.len() < MAX_RECORD_BYTES);
    let ctx = context(&[
        ("merge_loop_result", loop_line("DONE")),
        ("finalize_merge", merge_json("merged", "false")),
        ("autodrive_finalization", Value::String(text.clone())),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert_eq!(report["summary"].as_str(), Some(text.as_str()));
    assert!(report.get("summary_truncated").is_none());
}

#[test]
fn report_cuts_an_oversized_summary_on_a_character_boundary() {
    let auth = authorization("false", "false");
    // '€' is three bytes and MAX_RECORD_BYTES is not a multiple of three, so a
    // byte cut at the limit would split a character.
    let text = "€".repeat(MAX_RECORD_BYTES / 3 + 10);
    assert!(text.len() > MAX_RECORD_BYTES);
    assert_ne!(MAX_RECORD_BYTES % 3, 0);
    let ctx = context(&[
        ("merge_loop_result", loop_line("DONE")),
        ("finalize_merge", merge_json("merged", "false")),
        ("autodrive_finalization", Value::String(text.clone())),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    let summary = report["summary"].as_str().expect("summary is text");
    assert!(summary.len() <= MAX_RECORD_BYTES, "{}", summary.len());
    assert!(!summary.is_empty());
    assert!(text.starts_with(summary), "the cut keeps a prefix");
    assert!(summary.chars().all(|c| c == '€'), "no split character");
    assert_eq!(report["summary_truncated"], json!(true));
}

#[test]
fn report_note_names_no_file_contents() {
    let auth = authorization("false", "false");
    let secret = "ghp_FIXTURESECRETVALUE0123456789";
    let ctx = context(&[
        ("merge_loop_result", loop_line("DONE")),
        (
            "finalize_merge",
            Value::String(format!("{{\"result\": broken {secret}")),
        ),
        ("autodrive_finalization", Value::String("text".into())),
    ]);
    let report = run(&auth, &ctx);
    assert_v2_shape(&report);
    assert!(
        !report.to_string().contains(secret),
        "unreadable tool output is never copied into the report: {report}"
    );
}
