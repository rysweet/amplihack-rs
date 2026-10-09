//! Execute unmodified canonical commands through the native shell renderer.
use super::{fixtures::*, source_fixture::NativeFixture};
use serde_json::json;

const SITES: &[(&str, &str)] = &[
    ("workflow-design", "step-06b-checkpoint-doc-review"),
    ("workflow-tdd", "step-08c-enforce-verdict"),
    ("workflow-precommit-test", "verification-terminal-evidence"),
    ("workflow-finalize", "finalizer-step-status"),
];

#[test]
fn canonical_native_verification_accepts_one_object_or_string_receipt_only() {
    use super::producers::{precommit, testing};
    let f = NativeFixture::new();
    for (variant, receipts, expected) in [
        (
            "object-receipts",
            json!({"precommit_results":precommit(),
            "local_testing_gate":testing()}),
            "true",
        ),
        (
            "string-receipts",
            json!({"precommit_results":precommit().to_string(),
            "local_testing_gate":testing().to_string()}),
            "true",
        ),
        (
            "failure-then-pass",
            json!({"precommit_results":format!("{}\n{}",
            json!({"status":"FAIL","exit_code":101}),precommit()),
            "local_testing_gate":testing()}),
            "false",
        ),
        (
            "qa-failure-then-pass",
            json!({"precommit_results":precommit(),
            "local_testing_gate":format!("{}\n{}",
            json!({"status":"FAIL","exit_code":1}),testing())}),
            "false",
        ),
    ] {
        let result = f.run(
            "workflow-precommit-test",
            "verification-terminal-evidence",
            variant,
            receipts,
        );
        assert_eq!(result.code, 0, "{}", result.stderr);
        let output = result.json();
        let evidence = &output["context"]["verification_terminal_evidence"];
        assert_eq!(evidence["verification_completed"], expected, "{variant}");
        assert_eq!(evidence["terminal_no_op"], "false");
        assert_eq!(evidence.as_object().unwrap().len(), 4);
        assert!(
            evidence
                .as_object()
                .unwrap()
                .values()
                .all(|v| v.is_string())
        );
    }
}

#[test]
fn canonical_native_sources_select_private_framework_for_all_targets() {
    let f = NativeFixture::new();
    for (recipe, id) in SITES {
        for variant in ["main", "ordinary", "target-metachar", "framework-metachar"] {
            let result = f.run(recipe, id, variant, json!({}));
            assert_eq!(result.code, 0, "{recipe}/{variant}: {}", result.stderr);
            let output = result.json();
            assert_eq!(output["success"], true);
            assert_eq!(output["step_results"].as_array().unwrap().len(), 1);
            assert_eq!(output["step_results"][0]["step_id"], *id);
            assert_eq!(output["step_results"][0]["status"], "completed");
            let selected = &output["context"][step(recipe, id)["output"].as_str().unwrap()];
            match *recipe {
                "workflow-design" => assert!(
                    selected
                        .as_str()
                        .unwrap()
                        .contains("DOC_REVIEW_CHECKPOINT: OK")
                ),
                "workflow-tdd" => {
                    assert!(result.child_stderr.contains("work-verifier APPROVED"));
                    assert!(!result.child_stderr.contains("INSUFFICIENT_EVIDENCE"));
                }
                "workflow-precommit-test" => {
                    assert_eq!(selected["verification_completed"], "false");
                    assert_eq!(selected["terminal_no_op"], "false");
                    assert_eq!(selected["terminal_state"], "VERIFICATION_UNPROVEN");
                    assert_eq!(selected.as_object().unwrap().len(), 4);
                    assert!(
                        selected
                            .as_object()
                            .unwrap()
                            .values()
                            .all(|v| v.is_string())
                    );
                }
                "workflow-finalize" => assert_eq!(
                    selected,
                    &json!({"status":"ok","reporting_failure":"false"})
                ),
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn canonical_native_missing_helpers_do_not_use_installed_assets() {
    let f = NativeFixture::new();
    for (recipe, id) in SITES {
        let result = f.run(recipe, id, "missing", json!({}));
        assert!(result.child_stderr.contains("workflow_context.sh"));
        assert!(!result.child_stderr.contains("RECIPE_VAR_"));
        let output = result.json();
        if *recipe == "workflow-design" {
            assert_eq!(result.code, 0);
            let checkpoint = output["context"]["doc_review_checkpoint"].as_str().unwrap();
            assert!(checkpoint.contains("NEEDS_ATTENTION"));
            assert!(!checkpoint.contains("DOC_REVIEW_CHECKPOINT: OK"));
        } else {
            assert_ne!(result.code, 0);
            assert_eq!(output["success"], false);
        }
    }
}

#[test]
fn canonical_native_post_source_negative_policies_are_reached() {
    let f = NativeFixture::new();
    let hollow = f.run(
        "workflow-tdd",
        "step-08c-enforce-verdict",
        "hollow",
        json!({"verdict_json":{"verdict":"HOLLOW_SUCCESS","evidence":[]}}),
    );
    assert_eq!(hollow.code, 1);
    assert!(hollow.child_stderr.contains("HOLLOW_SUCCESS"));
    let docs = f.run(
        "workflow-design",
        "step-06b-checkpoint-doc-review",
        "negative",
        json!({"doc_review_feedback":{"status":"FAILED"}}),
    );
    assert_eq!(docs.code, 0);
    assert!(
        docs.json()["context"]["doc_review_checkpoint"]
            .as_str()
            .unwrap()
            .contains("NEEDS_ATTENTION")
    );
    let verification = f.run(
        "workflow-precommit-test",
        "verification-terminal-evidence",
        "failed",
        json!({"precommit_results":{"status":"FAIL","exit_code":101,"failed":22},
            "local_testing_gate":{"status":"PASS","exit_code":0}}),
    );
    assert_eq!(verification.code, 0);
    assert_eq!(
        verification.json()["context"]["verification_terminal_evidence"]["verification_completed"],
        "false"
    );
}
