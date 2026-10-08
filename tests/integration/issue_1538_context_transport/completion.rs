use super::fixtures::*;
use serde_json::json;

#[test]
fn complete_file_result_preserves_every_validated_field() {
    let f = Fixture::new()
        .env("IMPLEMENTATION_COMPLETED", "true")
        .env("VERIFICATION_COMPLETED", "true");
    let r = f.finalizer("validate");
    r.exit(0);
    let result = r.json();
    let out = Fixture::new()
        .file(json!({"workflow_result":result}))
        .env("WORKFLOW_RESULT_TERMINAL_SUCCESS", "false")
        .finalizer("complete");
    out.exit(0);
    let j = out.json();
    for (key, value) in result.as_object().unwrap() {
        if key == "pr_url" {
            assert_eq!(&j[key], value);
        } else {
            assert_eq!(&j["workflow_result"][key], value, "lost result field {key}");
        }
    }
}
#[test]
fn complete_canonical_negative_result_never_revives_stale_success() {
    let r = Fixture::new()
        .env(
            "RECIPE_VAR_workflow_result",
            json!({"terminal_success":"false","terminal_state":"FAILED_IMPLEMENTATION"})
                .to_string(),
        )
        .env("WORKFLOW_RESULT_TERMINAL_SUCCESS", "true")
        .env("WORKFLOW_RESULT_TERMINAL_STATE", "IMPLEMENTED_VERIFIED")
        .finalizer("complete");
    r.exit(0);
    assert_eq!(r.json()["terminal_success"], "false");
    assert_eq!(r.json()["terminal_state"], "FAILED_IMPLEMENTATION");
}

#[test]
fn complete_invalid_or_missing_canonical_result_never_borrows_success() {
    for raw in ["", "null", "{}", "{bad"] {
        let r = Fixture::new()
            .env("RECIPE_VAR_workflow_result", raw)
            .env("WORKFLOW_RESULT_TERMINAL_SUCCESS", "true")
            .env("WORKFLOW_RESULT_TERMINAL_STATE", "IMPLEMENTED_VERIFIED")
            .finalizer("complete");
        r.exit(0);
        assert_eq!(r.json()["terminal_success"], "false");
    }
}

#[test]
fn complete_file_negative_beats_stale_canonical_and_legacy_success() {
    let r = Fixture::new().file(json!({"workflow_result":{"terminal_success":"false","terminal_state":"FAILED_IMPLEMENTATION"}}))
        .env("RECIPE_VAR_workflow_result__terminal_success","true")
        .env("WORKFLOW_RESULT_TERMINAL_SUCCESS","true").finalizer("complete");
    r.exit(0);
    assert_eq!(r.json()["terminal_success"], "false");
    assert_eq!(r.json()["terminal_state"], "FAILED_IMPLEMENTATION");
}

#[test]
fn complete_legacy_only_result_aliases_remain_supported() {
    let r = Fixture::new()
        .env("WORKFLOW_RESULT_TERMINAL_SUCCESS", "true")
        .env("WORKFLOW_RESULT_TERMINAL_STATE", "IMPLEMENTED_VERIFIED")
        .env("WORKFLOW_RESULT_TERMINAL_REASON", "typed durable result")
        .env(
            "WORKFLOW_RESULT_PR_URL",
            "https://github.com/rysweet/amplihack-rs/pull/1540",
        )
        .finalizer("complete");
    r.exit(0);
    assert_eq!(r.json()["terminal_success"], "true");
    assert_eq!(r.json()["terminal_state"], "IMPLEMENTED_VERIFIED");
    assert_eq!(
        r.json()["workflow_result"]["terminal_reason"],
        "typed durable result"
    );
    assert_eq!(
        r.json()["pr_url"],
        "https://github.com/rysweet/amplihack-rs/pull/1540"
    );
}
