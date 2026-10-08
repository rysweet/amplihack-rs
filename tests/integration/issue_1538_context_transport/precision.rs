use super::fixtures::*;
use serde_json::json;

fn selected(channel: &str, key: &str, value: &str) -> Fixture {
    match channel {
        "file" => Fixture::new().file(json!({key:value})),
        "canonical" => Fixture::new().env(&format!("RECIPE_VAR_{key}"), value),
        "legacy" => Fixture::new().env(&key.to_uppercase(), value),
        _ => panic!("unknown channel"),
    }
}

#[test]
fn inherited_alternate_cohort_cannot_revive_legacy_noop() {
    for cohort in ["documentation", "verification", "metadata", "finalization"] {
        let f = Fixture::new()
            .env("RECIPE_VAR_verdict_json", verdict("HOLLOW_SUCCESS"))
            .env("WORKFLOW_CONTEXT_COHORT", cohort)
            .env("ALLOW_NO_OP", "true")
            .env("IMPLEMENTATION", SENTINEL);
        f.helper().helper_state("IMPLEMENTATION_UNPROVEN");
        f.gate().gate("HOLLOW_SUCCESS");
    }
}

#[test]
fn newline_noop_strings_keep_each_consumers_original_policy() {
    for channel in ["legacy", "canonical", "file"] {
        let f = selected(channel, "allow_no_op", "true\n").env(
            if channel == "legacy" {
                "VERDICT_JSON"
            } else {
                "RECIPE_VAR_verdict_json"
            },
            verdict("HOLLOW_SUCCESS"),
        );
        // Use a coherent complete file for the file case.
        let f = if channel == "file" {
            f.file(json!({"allow_no_op":"true\n","verdict_json":
                serde_json::from_str::<serde_json::Value>(&verdict("HOLLOW_SUCCESS")).unwrap()}))
        } else {
            f
        };
        f.gate().gate("HOLLOW_SUCCESS");
        f.helper().helper_state("ALLOW_NO_OP");
        let r = f.run(&body(
            "workflow-precommit-test",
            "verification-terminal-evidence",
        ));
        r.exit(0);
        assert_eq!(r.json()["terminal_no_op"], "true", "{channel}");
    }
}

#[test]
fn newline_only_verification_strings_remain_nonempty() {
    for channel in ["legacy", "canonical", "file"] {
        let f = selected(channel, "precommit_results", "\n");
        let f = if channel == "file" {
            f.file(json!({"precommit_results":"\n","local_testing_gate":"\n"}))
        } else {
            f.env(
                if channel == "legacy" {
                    "LOCAL_TESTING_GATE"
                } else {
                    "RECIPE_VAR_local_testing_gate"
                },
                "\n",
            )
        };
        let r = f.run(&body(
            "workflow-precommit-test",
            "verification-terminal-evidence",
        ));
        r.exit(0);
        assert_eq!(r.json()["verification_completed"], "true", "{channel}");
    }
}

#[test]
fn newline_completion_tokens_do_not_become_true() {
    for channel in ["legacy", "canonical", "file"] {
        let f = match channel {
            "file" => Fixture::new().file(json!({"implementation_terminal_evidence":{
                "implementation_completed":"true\n","terminal_no_op":"true\n"},
                "verification_terminal_evidence":{"verification_completed":"true\n"},"allow_no_op":"true\n"})),
            "canonical" => Fixture::new()
                .env("RECIPE_VAR_implementation_terminal_evidence", json!({"implementation_completed":"true\n","terminal_no_op":"true\n"}).to_string())
                .env("RECIPE_VAR_verification_terminal_evidence", json!({"verification_completed":"true\n"}).to_string())
                .env("RECIPE_VAR_allow_no_op", "true\n"),
            _ => Fixture::new().env("IMPLEMENTATION_COMPLETED", "true\n")
                .env("VERIFICATION_COMPLETED", "true\n").env("TERMINAL_NO_OP", "true\n").env("ALLOW_NO_OP", "true\n"),
        };
        let r = f.finalizer("collect");
        r.exit(0);
        for key in [
            "implementation_completed",
            "verification_completed",
            "terminal_no_op",
            "allow_no_op",
        ] {
            assert_eq!(r.json()["completion"][key], "false", "{channel} {key}");
        }
    }
}
