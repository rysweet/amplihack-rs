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

// #1726: an object-typed key declared with an empty default and not yet set
// reads as absent (status 2, nothing on either stream), not as invalid.
#[test]
fn reader_unset_empty_object_key_is_absent_not_invalid() {
    for key in ["worktree_setup", "pr_publish_result", "verdict_json"] {
        let r = Fixture::new()
            .env(&format!("RECIPE_VAR_{key}"), "")
            .run(&script(&format!("workflow_context_read {key}")));
        r.exit(2);
        assert!(r.stdout.is_empty(), "{key}: {}", r.stdout);
        assert!(r.stderr.is_empty(), "{key}: {}", r.stderr);
    }
}

#[test]
fn reader_empty_scalar_key_still_reads_as_empty_success() {
    let r = Fixture::new()
        .env("RECIPE_VAR_branch_name", "")
        .run(&script("workflow_context_read branch_name"));
    r.exit(0);
    assert!(r.stdout.is_empty());
}

// A file that was selected and carried nothing is still absence for every key
// -- status 2, nothing on stdout, no stale alias consulted -- and since #2819
// it also says so, once, naming the path and why. Without that line a consumer
// downstream reports only that its producer "produced no output" and nothing
// anywhere names the file that went missing. The wording is not asserted; the
// path, the variable and the status are.
#[test]
fn reader_missing_context_file_is_absent_for_every_key_kind_and_says_why() {
    const PATH: &str = "/missing/issue2343/context.json";
    for key in ["verdict_json", "worktree_setup", "branch_name"] {
        let r = Fixture::new()
            .env("AMPLIHACK_CONTEXT_FILE", PATH)
            .run(&script(&format!("workflow_context_read {key}")));
        r.exit(2);
        assert!(r.stdout.is_empty());
        assert!(
            r.stderr.contains(PATH) && r.stderr.contains("AMPLIHACK_CONTEXT_FILE"),
            "{key}: the warning must name the variable and the path: {}",
            r.stderr
        );
        assert!(
            !r.stderr.contains("invalid authoritative workflow context"),
            "{key}: it is absence, not invalid data: {}",
            r.stderr
        );
        assert_eq!(
            r.stderr.lines().filter(|l| l.contains(PATH)).count(),
            1,
            "{key}: the file's state is decided once, so it is reported once: {}",
            r.stderr
        );
    }
}

// #1726 through the context file. The file carries every value as a string, so
// an object-typed key declared with an empty default and never set arrives as
// "". The RECIPE_VAR read of that same key reports absence, and the file must
// not disagree: a run whose context file carries one must not end on "invalid
// authoritative workflow context" (#2343).
#[test]
fn reader_empty_object_value_in_the_file_is_absent_not_invalid() {
    for key in ["verdict_json", "worktree_setup", "finalization_assessment"] {
        let mut context = serde_json::Map::new();
        context.insert(key.to_owned(), json!(""));
        let r = Fixture::new()
            .file(serde_json::Value::Object(context))
            .run(&script(&format!("workflow_context_read {key}")));
        r.exit(2);
        assert!(r.stdout.is_empty(), "{key}: {}", r.stdout);
        assert!(r.stderr.is_empty(), "{key}: {}", r.stderr);
    }
}

#[test]
fn reader_empty_scalar_value_in_the_file_still_reads_as_empty_success() {
    for key in ["branch_name", "precommit_results"] {
        let mut context = serde_json::Map::new();
        context.insert(key.to_owned(), json!(""));
        let r = Fixture::new()
            .file(serde_json::Value::Object(context))
            .run(&script(&format!("workflow_context_read {key}")));
        r.exit(0);
        assert!(r.stdout.is_empty(), "{key}: {}", r.stdout);
    }
}

#[test]
fn reader_unparseable_object_value_in_the_file_is_still_invalid() {
    let r = Fixture::new()
        .file(json!({"verdict_json": "not-an-object"}))
        .run(&script("workflow_context_read verdict_json"));
    r.exit(3);
    assert!(r.stdout.is_empty());
}
