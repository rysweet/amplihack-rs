//! #2575 and #3129: the tolerant, schema-version-2 run report.
//!
//! Contract (docs/reference/autodrive-goal-loop.md#reported-outcomes):
//!
//! - `report(auth, context, expected_pr)` never fails and never panics. A
//!   badly shaped agent output cannot fail the run report.
//! - Fields: `schema_version` (always 2), `run_id`, `requested_action`,
//!   `merge`, `loop_result` (always null), `loop_end`, `after_merge_steps`,
//!   `stop_reason`, `summary`, and optionally `note` and `summary_truncated`.
//!   No readiness-record fields remain.
//! - `stop_reason` comes only from `loop-end.json` and `merge` only from
//!   `merge-result.json`, both in the runner-bound state folder and bound to
//!   this run. Recipe context never fills either. `summary` is
//!   `autodrive_finalization` verbatim, cut on a character boundary at
//!   `MAX_RECORD_BYTES`.
//!
//! The goal-loop rules are in `report_goal_tests.rs`. This file keeps the
//! #2575 cases that still describe kept behaviour, pointed at the records.
//! Registered from `orch.rs` next to the #2329 contract tests.

use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization, identity::MAX_RECORD_BYTES, report::report,
    state::EvidenceStore,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::OpenOptionsExt;

const HEAD: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");

const ALLOWED_KEYS: [&str; 11] = [
    "schema_version",
    "run_id",
    "requested_action",
    "merge",
    "loop_result",
    "loop_end",
    "after_merge_steps",
    "stop_reason",
    "summary",
    "note",
    "summary_truncated",
];

/// Every result the merge action writes other than `merged`.
const NOT_MERGED_RESULTS: [&str; 6] = [
    "not-authorised",
    "authorization-unreadable",
    "checks-not-green",
    "refused",
    "not-open",
    "message-unavailable",
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

/// A run with a private state folder bound to its invocation.
struct Bound {
    _temp: tempfile::TempDir,
    auth: InvocationAuthorization,
}

fn bound(no_merge: &str, dry_run: &str) -> Bound {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("state");
    EvidenceStore::create(&dir).unwrap();
    let mut auth = authorization(no_merge, dry_run);
    auth.bind_state_dir(dir);
    Bound { _temp: temp, auth }
}

impl Bound {
    fn put_raw(&self, name: &str, bytes: &[u8]) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.auth.state_dir().unwrap().join(name))
            .unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
    }
    fn put(&self, name: &str, body: &Value) {
        self.put_raw(name, format!("{body}\n").as_bytes());
    }
    /// The driver's `loop-end.json` for this run.
    fn end(&self, word: &str) {
        self.put(
            "loop-end.json",
            &json!({"end": word, "run_id": self.auth.run_id(), "subject": "PR 2575", "detail": "fixture"}),
        );
    }
    /// The merge action's `merge-result.json` for this run.
    fn merge(&self, result: &str, handoff: &str) {
        self.put(
            "merge-result.json",
            &json!({"result": result, "pr": 2575, "expected_head": HEAD, "handoff": handoff,
                "reason": "fixture", "run_id": self.auth.run_id()}),
        );
    }
    fn run(&self, ctx: &Map<String, Value>) -> Value {
        report(&self.auth, ctx, Some("2575"))
    }
}

fn context(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn finalisation(text: &str) -> Map<String, Value> {
    context(&[("autodrive_finalization", Value::String(text.into()))])
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
    for key in [
        "run_id",
        "requested_action",
        "merge",
        "loop_result",
        "loop_end",
        "after_merge_steps",
        "stop_reason",
        "summary",
    ] {
        assert!(object.contains_key(key), "missing `{key}` in {report}");
    }
    assert!(object["loop_result"].is_null(), "{report}");
    assert!(object["after_merge_steps"].is_array(), "{report}");
}

#[test]
fn report_v2_merged_end_is_completed_with_verbatim_summary() {
    let run = bound("false", "false");
    run.merge("merged", "false");
    run.end("merged");
    let summary = "PR #2575 merged at 15a7f6f4 after all 13 criteria were met.";
    let report = run.run(&finalisation(summary));
    assert_v2_shape(&report);
    assert_eq!(report["run_id"], json!(run.auth.run_id()));
    assert_eq!(report["requested_action"], json!("merge"));
    assert_eq!(report["loop_end"], json!("merged"));
    assert_eq!(report["stop_reason"], json!("completed"));
    assert_eq!(report["merge"]["result"], json!("merged"));
    assert_eq!(report["merge"]["pr"], json!(2575));
    assert_eq!(report["merge"]["expected_head"], json!(HEAD));
    assert_eq!(report["merge"]["handoff"], json!("false"));
    assert_eq!(report["summary"], json!(summary));
    assert!(report.get("summary_truncated").is_none(), "{report}");
}

#[test]
fn report_v2_ready_end_of_a_verify_only_run_is_completed() {
    // #2575 B4: a verify-only run (no_merge or dry run) that reaches its
    // readiness end did what it was asked and is `completed`; only a merge
    // run must merge. Its merge action, had the agent chosen it, refused.
    for (no_merge, dry_run) in [("true", "false"), ("false", "true"), ("true", "true")] {
        let run = bound(no_merge, dry_run);
        run.merge("not-authorised", "false");
        run.end("ready");
        let report = run.run(&finalisation("Verified ready; no merge was requested."));
        assert_v2_shape(&report);
        assert_eq!(report["requested_action"], json!("verify_readiness"));
        assert_eq!(report["stop_reason"], json!("completed"), "{report}");
        assert_eq!(report["loop_end"], json!("ready"), "{report}");
        assert_eq!(report["merge"]["result"], json!("not-authorised"));
    }
}

#[test]
fn report_v2_refused_merge_results_are_echoed_and_never_complete() {
    for result in NOT_MERGED_RESULTS {
        let run = bound("false", "false");
        run.merge(result, "true");
        run.end("handoff");
        let report = run.run(&finalisation(&format!("Not merged: {result}.")));
        assert_v2_shape(&report);
        assert_eq!(
            report["stop_reason"],
            json!("handoff"),
            "{result}: {report}"
        );
        assert_eq!(report["merge"]["result"], json!(result));
        assert_eq!(report["merge"]["handoff"], json!("true"));
    }
}

#[test]
fn report_v2_not_authorised_on_a_merge_run_is_never_completed() {
    for end in ["handoff", "ready"] {
        let run = bound("false", "false");
        run.merge("not-authorised", "false");
        run.end(end);
        let report = run.run(&finalisation("Not merged."));
        assert_v2_shape(&report);
        assert_eq!(report["requested_action"], json!("merge"));
        assert_eq!(report["stop_reason"], json!("handoff"), "{end}: {report}");
        assert_eq!(report["merge"]["result"], json!("not-authorised"));
    }
}

#[test]
fn report_v2_no_loop_end_is_failed_with_a_note() {
    let run = bound("false", "false");
    let with_folder = run.run(&finalisation(""));
    assert_v2_shape(&with_folder);
    assert_eq!(with_folder["stop_reason"], json!("failed"), "{with_folder}");
    assert!(with_folder["merge"].is_null(), "{with_folder}");
    assert!(with_folder["loop_end"].is_null(), "{with_folder}");
    assert!(
        with_folder["note"]
            .as_str()
            .is_some_and(|n| n.contains("loop-end.json")),
        "an unread loop end is named in a note: {with_folder}"
    );

    // No state folder bound at all: nothing to read, and still a note.
    let unbound = report(&authorization("false", "false"), &Map::new(), Some("2575"));
    assert_v2_shape(&unbound);
    assert_eq!(unbound["stop_reason"], json!("failed"), "{unbound}");
    assert!(
        unbound["note"].as_str().is_some_and(|n| !n.is_empty()),
        "{unbound}"
    );
}

#[test]
fn report_v2_stop_reason_mapping() {
    for (end, expected) in [
        ("merged", "completed"),
        ("handoff", "handoff"),
        ("closed", "closed"),
        ("stopped", "stopped"),
        ("refused", "terminal_refusal"),
        ("failed", "failed"),
        ("usage_limit", "usage_limit"),
        ("orphaned", "orphaned"),
    ] {
        let run = bound("false", "false");
        run.end(end);
        let report = run.run(&finalisation(&format!("Stopped: {end}.")));
        assert_v2_shape(&report);
        assert_eq!(report["stop_reason"], json!(expected), "{end}: {report}");
        assert_eq!(report["loop_end"], json!(end), "{report}");
        assert!(report["merge"].is_null(), "{end}: {report}");
    }
}

#[test]
fn report_v2_merge_comes_only_from_tool_json_not_from_agent_prose() {
    // The agent's finalisation statement claims a merge; the driver says the
    // loop handed off and no merge action ran. The prose decides nothing.
    let prose = r#"{"result":"merged","stop_reason":"completed","end":"merged"} The PR merged."#;
    let run = bound("false", "false");
    run.end("handoff");
    let report = run.run(&finalisation(prose));
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("handoff"), "{report}");
    assert!(report["merge"].is_null(), "{report}");

    let run = bound("false", "false");
    let report = run.run(&finalisation(prose));
    assert_v2_shape(&report);
    assert_ne!(report["stop_reason"], json!("completed"), "{report}");
    assert!(report["merge"].is_null(), "{report}");
}

#[test]
fn report_tolerates_any_finalisation_text() {
    for text in [
        FIXTURE_UNKNOWN_KEYS,
        FIXTURE_NOTICE_FIRST,
        FIXTURE_NULL_CAUSE,
        "",
        "{",
        "null",
        "\u{0}\u{1b}[31mcontrol bytes\u{7f}",
    ] {
        let run = bound("false", "false");
        run.merge("merged", "false");
        run.end("merged");
        let report = run.run(&finalisation(text));
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
                let report = report(&auth, &ctx, Some("2575"));
                assert_v2_shape(&report);
                assert_ne!(
                    report["stop_reason"],
                    json!("completed"),
                    "context without a loop end never reports completion: {report}"
                );
            }
        }
    }
    // The same odd values beside this run's records change nothing the
    // records decide, and still never panic.
    let run = bound("false", "false");
    run.merge("merged", "false");
    run.end("merged");
    for value in &odd {
        let ctx = context(&[
            ("merge_loop_result", value.clone()),
            ("finalize_merge", value.clone()),
            ("crusty_loop_result", value.clone()),
            ("autodrive_finalization", value.clone()),
        ]);
        let report = run.run(&ctx);
        assert_v2_shape(&report);
        assert_eq!(report["stop_reason"], json!("completed"), "{report}");
        assert_eq!(report["merge"]["result"], json!("merged"), "{report}");
        assert!(report["summary"].is_string(), "{report}");
    }
}

#[test]
fn report_keeps_a_two_mebibyte_multibyte_summary_whole() {
    let run = bound("false", "false");
    run.end("merged");
    let text = "é".repeat(1024 * 1024); // 2 MiB
    assert!(text.len() < MAX_RECORD_BYTES);
    let report = run.run(&finalisation(&text));
    assert_v2_shape(&report);
    assert_eq!(report["summary"].as_str(), Some(text.as_str()));
    assert!(report.get("summary_truncated").is_none());
}

#[test]
fn report_cuts_an_oversized_summary_on_a_character_boundary() {
    let run = bound("false", "false");
    run.end("merged");
    // '€' is three bytes and MAX_RECORD_BYTES is not a multiple of three, so a
    // byte cut at the limit would split a character.
    let text = "€".repeat(MAX_RECORD_BYTES / 3 + 10);
    assert!(text.len() > MAX_RECORD_BYTES);
    assert_ne!(MAX_RECORD_BYTES % 3, 0);
    let report = run.run(&finalisation(&text));
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
    let secret = "ghp_FIXTURESECRETVALUE0123456789";
    // A broken merge record and a loop end whose word is not a word.
    let run = bound("false", "false");
    run.put_raw(
        "merge-result.json",
        format!("{{\"result\": broken {secret}").as_bytes(),
    );
    run.put(
        "loop-end.json",
        &json!({"end": secret, "run_id": run.auth.run_id(), "subject": secret, "detail": secret}),
    );
    let report = run.run(&finalisation("text"));
    assert_v2_shape(&report);
    assert!(
        !report.to_string().contains(secret),
        "unreadable tool output is never copied into the report: {report}"
    );
    assert_eq!(report["stop_reason"], json!("failed"), "{report}");
    assert!(report["loop_end"].is_null(), "{report}");

    // A loop end that is not JSON at all.
    let run = bound("false", "false");
    run.put_raw(
        "loop-end.json",
        format!("end: merged {secret}\n").as_bytes(),
    );
    let report = run.run(&finalisation("text"));
    assert_v2_shape(&report);
    assert!(!report.to_string().contains(secret), "{report}");
    assert_eq!(report["stop_reason"], json!("failed"), "{report}");
}

#[test]
fn report_v2_unknown_merge_result_is_never_echoed_or_completed() {
    for result in [
        "bogus",
        "MERGED",
        "merged ",
        "completed",
        "already-merged",
        "checks-pending",
        "base-behind",
    ] {
        let run = bound("false", "false");
        run.merge(result, "false");
        run.end("handoff");
        let report = run.run(&finalisation("merged"));
        assert_v2_shape(&report);
        assert_eq!(
            report["stop_reason"],
            json!("handoff"),
            "an unknown result decides nothing: {result:?}: {report}"
        );
        assert!(report["merge"].is_null(), "{result:?}: {report}");
        assert!(
            report["note"]
                .as_str()
                .is_some_and(|n| n.contains("merge-result.json")),
            "{result:?}: {report}"
        );
    }
}

#[test]
fn report_v2_a_merge_record_never_completes_a_run_by_itself() {
    for result in NOT_MERGED_RESULTS
        .iter()
        .chain(&["merged", "checks-pending", "base-behind"])
    {
        let run = bound("false", "false");
        run.merge(result, "false");
        let report = run.run(&Map::new());
        assert_v2_shape(&report);
        assert_ne!(
            report["stop_reason"],
            json!("completed"),
            "{result}: {report}"
        );
    }
}

#[test]
fn report_v2_merged_end_without_this_runs_merge_is_completed_with_a_note() {
    // The driver read the PR MERGED. Whatever the merge action said, the run
    // is completed, and the note says the merge was not by its merge action.
    for result in NOT_MERGED_RESULTS {
        let run = bound("false", "false");
        run.merge(result, "false");
        run.end("merged");
        let report = run.run(&Map::new());
        assert_v2_shape(&report);
        assert_eq!(report["stop_reason"], json!("completed"), "{report}");
        assert_eq!(report["merge"]["result"], json!(result), "{report}");
        assert!(
            report["note"]
                .as_str()
                .is_some_and(|n| n.contains("not by this run's merge action")),
            "{result}: {report}"
        );
    }
}

#[test]
fn report_v2_merged_record_without_pr_is_not_trusted() {
    // The merge action always writes `pr`; a record without one, read while a
    // PR is expected, is not echoed and never completes the run.
    let run = bound("false", "false");
    run.put(
        "merge-result.json",
        &json!({"result":"merged","expected_head":HEAD,"handoff":"false","run_id":run.auth.run_id()}),
    );
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_ne!(report["stop_reason"], json!("completed"), "{report}");
    assert!(report["merge"].is_null(), "{report}");

    // Another PR's record is not echoed either.
    let run = bound("false", "false");
    run.put(
        "merge-result.json",
        &json!({"result":"merged","pr":2576,"expected_head":HEAD,"handoff":"false","run_id":run.auth.run_id()}),
    );
    run.end("handoff");
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("handoff"), "{report}");
    assert!(report["merge"].is_null(), "{report}");
}

#[test]
fn report_v2_reads_this_runs_records_from_the_state_folder() {
    let run = bound("false", "false");
    run.merge("merged", "false");
    run.end("merged");
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("completed"), "{report}");
    assert_eq!(report["merge"]["result"], json!("merged"), "{report}");
    assert!(
        report.get("note").is_none(),
        "a merge by this run's merge action needs no note: {report}"
    );
}

#[test]
fn report_v2_ignores_another_runs_records() {
    const OTHER: &str = "11111111-2222-4333-8444-555555555555";
    let run = bound("false", "false");
    run.put(
        "merge-result.json",
        &json!({"result":"merged","pr":2575,"expected_head":HEAD,"handoff":"false","run_id":OTHER}),
    );
    run.end("merged");
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert!(report["merge"].is_null(), "{report}");
    assert_eq!(report["stop_reason"], json!("completed"), "{report}");
    assert!(
        report["note"]
            .as_str()
            .is_some_and(|n| n.contains("not by this run's merge action")),
        "{report}"
    );

    let run = bound("false", "false");
    run.put(
        "loop-end.json",
        &json!({"end":"merged","run_id":OTHER,"subject":"x","detail":"y"}),
    );
    run.merge("merged", "false");
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("failed"), "{report}");
    assert!(report["loop_end"].is_null(), "{report}");
}

#[test]
fn report_v2_never_reads_the_merge_from_the_context() {
    // The state folder's record is used; the context's is not.
    let run = bound("false", "false");
    run.merge("merged", "false");
    run.end("merged");
    let refused = json!({"result":"refused","pr":2575,"expected_head":HEAD,"handoff":"true"});
    let report = run.run(&context(&[(
        "finalize_merge",
        Value::String(refused.to_string()),
    )]));
    assert_v2_shape(&report);
    assert_eq!(report["merge"]["result"], json!("merged"), "{report}");
    assert_eq!(report["stop_reason"], json!("completed"), "{report}");

    // A context merge with no record in the state folder fills nothing.
    let run = bound("false", "false");
    run.end("handoff");
    let merged = json!({"result":"merged","pr":2575,"expected_head":HEAD,"handoff":"false",
        "run_id": run.auth.run_id()});
    let report = run.run(&context(&[
        ("finalize_merge", Value::String(merged.to_string())),
        (
            "merge_loop_result",
            json!({"loop_result":"DONE"}).to_string().into(),
        ),
    ]));
    assert_v2_shape(&report);
    assert!(report["merge"].is_null(), "{report}");
    assert_eq!(report["stop_reason"], json!("handoff"), "{report}");
}

#[test]
fn report_v2_after_merge_steps_are_capped_and_only_from_a_merged_end() {
    // At most 32 steps.
    let run = bound("false", "false");
    let many: Vec<String> = (0..40).map(|n| format!("step {n}")).collect();
    run.put(
        "loop-end.json",
        &json!({"end":"merged","run_id":run.auth.run_id(),"subject":"s","detail":"d","after_merge":many}),
    );
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("completed"), "{report}");
    assert_eq!(report["after_merge_steps"], json!(many[..32]), "{report}");
    assert!(
        report["note"]
            .as_str()
            .is_some_and(|n| n.contains("after_merge cut")),
        "{report}"
    );

    // At most 8 KiB in all; the list stops at the first step past the cap.
    let run = bound("false", "false");
    let big = "x".repeat(3000);
    run.put(
        "loop-end.json",
        &json!({"end":"merged","run_id":run.auth.run_id(),"subject":"s","detail":"d",
            "after_merge":[big, big, big, "short"]}),
    );
    let report = run.run(&Map::new());
    assert_eq!(report["after_merge_steps"], json!([big, big]), "{report}");
    let total: usize = report["after_merge_steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().len())
        .sum();
    assert!(total <= 8 * 1024, "{total}");

    // Values that are not text are left out, with a note.
    let run = bound("false", "false");
    run.put(
        "loop-end.json",
        &json!({"end":"merged","run_id":run.auth.run_id(),"subject":"s","detail":"d",
            "after_merge":["tag the release", 7, {"x": 1}, null, "close the issue"]}),
    );
    let report = run.run(&Map::new());
    assert_eq!(
        report["after_merge_steps"],
        json!(["tag the release", "close the issue"]),
        "{report}"
    );
    assert!(
        report["note"]
            .as_str()
            .is_some_and(|n| n.contains("not text")),
        "{report}"
    );

    // Not a list: nothing, and the run is still completed.
    let run = bound("false", "false");
    run.put(
        "loop-end.json",
        &json!({"end":"merged","run_id":run.auth.run_id(),"subject":"s","detail":"d","after_merge":"tag it"}),
    );
    let report = run.run(&Map::new());
    assert_eq!(report["after_merge_steps"], json!([]), "{report}");
    assert_eq!(report["stop_reason"], json!("completed"), "{report}");

    // Steps on any end other than `merged` are not reported.
    for end in ["handoff", "ready", "closed", "stopped", "failed"] {
        let run = bound("false", "false");
        run.put(
            "loop-end.json",
            &json!({"end":end,"run_id":run.auth.run_id(),"subject":"s","detail":"d","after_merge":["tag it"]}),
        );
        let report = run.run(&Map::new());
        assert_v2_shape(&report);
        assert_eq!(report["after_merge_steps"], json!([]), "{end}: {report}");
    }
}

#[test]
fn report_v2_reads_a_pretty_printed_record_and_refuses_two_objects() {
    // A record written by `jq` without `-c` spans several lines.
    let run = bound("false", "false");
    let end = json!({"end":"merged","run_id":run.auth.run_id(),"subject":"PR 2575","detail":"d"});
    run.put_raw(
        "loop-end.json",
        serde_json::to_string_pretty(&end).unwrap().as_bytes(),
    );
    let merge = json!({"result":"merged","pr":2575,"expected_head":HEAD,"handoff":"false",
        "run_id":run.auth.run_id()});
    run.put_raw(
        "merge-result.json",
        serde_json::to_string_pretty(&merge).unwrap().as_bytes(),
    );
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("completed"), "{report}");
    assert_eq!(report["merge"]["result"], json!("merged"), "{report}");
    assert!(report.get("note").is_none(), "{report}");

    // Two objects in one file are not one record, whichever comes last.
    let run = bound("false", "false");
    let handoff = json!({"end":"handoff","run_id":run.auth.run_id(),"subject":"s","detail":"d"});
    let merged = json!({"end":"merged","run_id":run.auth.run_id(),"subject":"s","detail":"d"});
    run.put_raw("loop-end.json", format!("{handoff}\n{merged}\n").as_bytes());
    let report = run.run(&Map::new());
    assert_v2_shape(&report);
    assert_eq!(report["stop_reason"], json!("failed"), "{report}");
    assert!(
        report["note"]
            .as_str()
            .is_some_and(|n| n.contains("loop-end.json: not one JSON object")),
        "{report}"
    );
}

#[test]
fn report_v2_never_creates_a_missing_state_folder() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("never-made");
    let mut auth = authorization("false", "false");
    auth.bind_state_dir(missing.clone());
    let outcome = report(&auth, &Map::new(), Some("2575"));
    assert_v2_shape(&outcome);
    assert_eq!(outcome["stop_reason"], json!("failed"), "{outcome}");
    assert!(!missing.exists(), "the report made {}", missing.display());

    // A state folder that is a link to a folder holding a merged end is not
    // followed.
    let real = temp.path().join("real");
    EvidenceStore::create(&real).unwrap();
    let mut auth = authorization("false", "false");
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    auth.bind_state_dir(link);
    let end = json!({"end":"merged","run_id":auth.run_id(),"subject":"s","detail":"d"});
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(real.join("loop-end.json"))
        .unwrap();
    std::io::Write::write_all(&mut file, end.to_string().as_bytes()).unwrap();
    let outcome = report(&auth, &Map::new(), Some("2575"));
    assert_v2_shape(&outcome);
    assert_ne!(outcome["stop_reason"], json!("completed"), "{outcome}");
}
