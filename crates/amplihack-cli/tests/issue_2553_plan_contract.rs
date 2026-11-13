//! Issue #2553, fault 2 — the plan a decomposition hands to the orchestrator
//! must never carry a fabricated issue number.
//!
//! `build_workstreams_config_*` minted `issue: "TBD"` on every entry, which
//! `WorkstreamConfig::issue_id()` read back as `0`, which tripped the
//! `Invalid issue number` bail in `orchestrator.rs`. Absence is now written as
//! absence: no placeholder string, no zero, no field.

use amplihack_cli::commands::orch::build_workstreams_config_to_tempfile;
use serde_json::{Value, json};

fn config_for(decomp: &Value) -> Value {
    let path = build_workstreams_config_to_tempfile(&decomp.to_string())
        .expect("the helper writes a workstreams-config");
    let body = std::fs::read_to_string(&path).expect("the config is readable");
    std::fs::remove_file(&path).ok();
    serde_json::from_str(&body).expect("the config is JSON")
}

fn entries(config: &Value) -> &Vec<Value> {
    config.as_array().expect("the config is an array")
}

#[test]
fn normalisation_preserves_leaf_identity_task_destination_dependencies_and_tracking_intent() {
    let config = config_for(&json!({"task_type": "Operations", "workstreams": [
        {"key": "research", "name": "Research", "description": "Answers for the status issue",
         "task": "Research the routing question and report in chat.", "classification": "Operations",
         "dependencies": [], "create_issue": true},
        {"key": "docs", "name": "Docs", "description": "Commit the agreed wording",
         "task": "Edit docs/routing.md with the research answers.", "classification": "Development",
         "dependencies": ["research"], "create_issue": false}
    ]}));
    let leaves = entries(&config);
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves[0]["key"], "research");
    assert_eq!(
        leaves[0]["task"],
        "Research the routing question and report in chat."
    );
    assert_eq!(leaves[0]["description"], "Answers for the status issue");
    assert_eq!(leaves[0]["classification"], "Operations");
    assert_eq!(leaves[0]["create_issue"], true);
    assert_eq!(leaves[1]["dependencies"], json!(["research"]));
    assert_eq!(leaves[1]["classification"], "Development");
    assert_eq!(leaves[1]["create_issue"], false);
}

#[test]
fn an_operations_label_cannot_hide_an_explicit_repository_edit_in_the_executable_task() {
    let config = config_for(&json!({"task_type": "Operations", "workstreams": [
        {"key": "edit", "name": "Coordinate the wording sweep", "description": "Project coordination",
         "task": "Correct the wording in tracked docs/routing.md and commit it.",
         "classification": "Operations", "recipe": "operations-direct"}
    ]}));
    assert_eq!(entries(&config)[0]["recipe"], "default-workflow");
    assert_eq!(entries(&config)[0]["classification"], "Development");
}

#[test]
fn invalid_plan_diagnostics_aggregate_independent_errors_with_keys_and_field_paths() {
    let error = build_workstreams_config_to_tempfile(&json!({"task_type": "Operations", "workstreams": [
        {"key": "empty-task", "name": "Empty", "task": "  ", "description": "  "},
        {"key": "bad-issue", "name": "Bad issue", "task": "Report status", "issue": 0},
        {"key": "missing-parent", "name": "Missing", "task": "Report status", "dependencies": ["absent"]}
    ]}).to_string()).expect_err("validate the whole plan before publishing it");
    let diagnostic = format!("{error:#}");
    for expected in [
        "empty-task",
        "task",
        "bad-issue",
        "issue",
        "missing-parent",
        "dependencies",
    ] {
        assert!(
            diagnostic.contains(expected),
            "missing {expected:?} in {diagnostic}"
        );
    }
}

#[test]
fn empty_or_malformed_decompositions_are_errors_instead_of_successful_empty_plans() {
    for input in [
        "not JSON",
        "{}",
        r#"{"workstreams":null}"#,
        r#"{"workstreams":[]}"#,
        r#"{"workstreams":[false]}"#,
    ] {
        let result = build_workstreams_config_to_tempfile(input);
        if let Ok(path) = &result {
            std::fs::remove_file(path).unwrap();
        }
        assert!(result.is_err(), "invalid decomposition accepted: {input}");
    }
}

/// A decomposition does not know issue numbers; nothing downstream may be
/// told that it does. The field is omitted, not set to a placeholder, so that
/// a consumer testing for presence cannot be fooled.
#[test]
fn a_decomposition_without_issues_writes_no_issue_field_at_all() {
    let config = config_for(&json!({
        "task_type": "Development",
        "workstreams": [
            {"name": "API service", "description": "Implement the REST API"},
            {"name": "Web UI", "description": "Build the React UI"}
        ]
    }));

    for entry in entries(&config) {
        assert!(
            entry.get("issue").is_none(),
            "an absent issue is an absent field, not a placeholder: {entry:#}"
        );
    }
    assert!(
        !config.to_string().contains("TBD"),
        "no placeholder string reaches the plan: {config:#}"
    );
}

/// The entry still carries everything the orchestrator needs to name and run
/// the workstream without an issue number.
#[test]
fn an_issue_less_entry_still_carries_its_identity_fields() {
    let config = config_for(&json!({
        "task_type": "Development",
        "workstreams": [{"name": "API service", "description": "Implement the REST API"}]
    }));
    let entry = &entries(&config)[0];

    assert_eq!(entry["description"], "API service");
    assert_eq!(entry["task"], "Implement the REST API");
    assert_eq!(entry["recipe"], "default-workflow");
    assert_eq!(entry["branch"], "feat/orch-1-api-service");
}

/// When the model does supply an issue, every accepted form is normalised to
/// the canonical integer at the point the plan is written, so no downstream
/// consumer has to re-parse `#N` or a URL.
#[test]
fn a_supplied_issue_is_normalised_to_the_canonical_integer() {
    for supplied in [
        json!(2553),
        json!("2553"),
        json!("002553"),
        json!("#2553"),
        json!("https://github.com/rysweet/amplihack-rs/issues/2553"),
    ] {
        let config = config_for(&json!({
            "task_type": "Development",
            "workstreams": [{"name": "x", "description": "Do the thing", "issue": supplied}]
        }));
        assert_eq!(entries(&config)[0]["issue"], json!(2553), "{supplied}");
    }
}

/// Every placeholder the field has been seen to carry reads as absence, not
/// as an error and not as zero.
#[test]
fn every_placeholder_form_is_written_back_as_absence() {
    for supplied in [
        Value::Null,
        json!(""),
        json!("   "),
        json!("TBD"),
        json!(" tbd "),
        json!("TBC"),
        json!("todo"),
        json!("N/A"),
        json!("none"),
        json!("null"),
        json!("Unknown"),
        json!("unassigned"),
        json!("pending"),
    ] {
        let config = config_for(&json!({
            "task_type": "Development",
            "workstreams": [{"name": "x", "description": "Do the thing", "issue": supplied}]
        }));
        assert!(
            entries(&config)[0].get("issue").is_none(),
            "{supplied} must read as absence"
        );
    }
}

/// Widening the absent set must not widen the present set. A value that is
/// neither a placeholder nor a well-formed issue is a producer mistake and
/// stays a failure, with a diagnostic that names the offending value.
#[test]
fn a_malformed_issue_is_a_diagnostic_failure_not_a_silent_absence() {
    for supplied in [
        json!("abc123"),
        json!("12x"),
        json!(-5),
        json!(0),
        json!("0"),
        json!(2.5),
        json!(true),
        json!([2553]),
        json!("https://attacker.example/rysweet/amplihack-rs/issues/1"),
        json!("https://github.com/rysweet/amplihack-rs/pull/2553"),
    ] {
        let decomp = json!({
            "task_type": "Development",
            "workstreams": [{"name": "x", "description": "Do the thing", "issue": supplied}]
        });
        let err = build_workstreams_config_to_tempfile(&decomp.to_string())
            .expect_err(&format!("{supplied} must not produce a plan"));
        let text = format!("{err:#}");
        assert!(
            text.contains("issue"),
            "the diagnostic must name the field: {text}"
        );
    }
}

/// `u64::MAX` is not a realistic issue number, but the type that carries it
/// is the point: the old path read the field through `i64`, so a value above
/// `i64::MAX` truncated to a negative number and then to zero.
#[test]
fn the_largest_representable_issue_survives_without_signed_truncation() {
    let config = config_for(&json!({
        "task_type": "Development",
        "workstreams": [{"name": "x", "description": "Do the thing", "issue": u64::MAX}]
    }));
    assert_eq!(entries(&config)[0]["issue"], json!(u64::MAX));
}

/// Issue #1966's `is_critical_code` pass-through is untouched by the issue
/// change; it shares the entry builder.
#[test]
fn criticality_still_passes_through_an_issue_less_entry() {
    let config = config_for(&json!({
        "task_type": "Development",
        "workstreams": [{"name": "x", "description": "Do the thing", "is_critical_code": true}]
    }));
    assert_eq!(entries(&config)[0]["is_critical_code"], json!("true"));
    assert!(entries(&config)[0].get("issue").is_none());
}
