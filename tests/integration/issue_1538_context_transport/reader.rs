//! API prerequisites are distinct from executed consumer semantic RED.
use super::fixtures::*;
use serde_json::json;

fn script(tail: &str) -> String {
    let p = repo().join("amplifier-bundle/tools/workflow_context.sh");
    assert!(
        p.is_file(),
        "planned reader API is absent; this is not consumer semantic RED"
    );
    format!("set -uo pipefail\n. '{}'\n{tail}\n", p.display())
}

#[test]
fn reader_round_trips_complete_object_with_one_compatibility_decode() {
    let object: serde_json::Value = serde_json::from_str(&verdict("WORK_VERIFIED")).unwrap();
    for raw in [object.to_string(), json!(object.to_string()).to_string()] {
        let r = Fixture::new()
            .env("RECIPE_VAR_verdict_json", raw)
            .run(&script("workflow_context_read verdict_json"));
        r.exit(0);
        assert_eq!(r.json(), object);
    }
}
#[test]
fn reader_false_is_usable_and_missing_field_returns_two() {
    let f = Fixture::new().file(json!({"allow_no_op":false}));
    let r = f.run(&script("workflow_context_read allow_no_op"));
    r.exit(0);
    assert_eq!(r.stdout.trim(), "false");
    let r = f.run(&script("workflow_context_read verdict_json"));
    r.exit(2);
    assert!(r.stdout.is_empty());
}
#[test]
fn reader_invalid_input_returns_three_without_payload_disclosure() {
    let secret = "SECRET_MARKER_1538 malformed {\"verdict\":\"WORK_VERIFIED\"}";
    let r = Fixture::new()
        .env("RECIPE_VAR_verdict_json", secret)
        .run(&script("workflow_context_read verdict_json"));
    r.exit(3);
    assert!(r.stdout.is_empty());
    assert!(!r.stderr.contains("SECRET_MARKER_1538"));
    assert!(r.stderr.len() <= 1024, "fixed bounded diagnostics required");
}
#[test]
fn reader_rejects_unsupported_key_with_empty_stdout() {
    let r = Fixture::new()
        .env("SECRET", "WORK_VERIFIED")
        .run(&script("workflow_context_read SECRET"));
    r.exit(3);
    assert!(r.stdout.is_empty());
}
#[test]
fn reader_holds_one_file_snapshot_across_direct_reads() {
    let f = Fixture::new().file(json!({"implementation":"first","allow_no_op":false}));
    let r = f.run(&script("workflow_context_read implementation\nprintf '\\n'\nprintf '%s' '{\"implementation\":\"replacement\",\"allow_no_op\":true}' > \"$AMPLIHACK_CONTEXT_FILE\"\nworkflow_context_read allow_no_op"));
    r.exit(0);
    assert_eq!(
        r.stdout.split_whitespace().collect::<Vec<_>>(),
        vec!["first", "false"]
    );
}
#[test]
fn reader_does_not_write_transport_aliases_or_completion_flags() {
    let r = Fixture::new().env("RECIPE_VAR_verdict_json",verdict("WORK_VERIFIED"))
        .run(&script("workflow_context_read verdict_json >/dev/null\n[ -z \"${VERDICT_JSON+x}${IMPLEMENTATION_COMPLETED+x}${VERIFICATION_COMPLETED+x}\" ]"));
    r.exit(0);
    assert!(r.stdout.is_empty());
}

#[test]
fn reader_freezes_canonical_presence_and_values_before_reads() {
    let r = Fixture::new().env("RECIPE_VAR_allow_no_op", "false")
        .env("ALLOW_NO_OP", "true")
        .run(&script("RECIPE_VAR_allow_no_op=true\nunset RECIPE_VAR_allow_no_op\nworkflow_context_read allow_no_op"));
    r.exit(0);
    assert_eq!(r.stdout, "false");
}

#[test]
fn reader_file_selection_survives_selector_changes() {
    let r = Fixture::new()
        .file(json!({"allow_no_op":false}))
        .env("ALLOW_NO_OP", "true")
        .run(&script(
            "unset AMPLIHACK_CONTEXT_FILE\nworkflow_context_read allow_no_op",
        ));
    r.exit(0);
    assert_eq!(r.stdout, "false");
}
