use super::{
    fixtures::*,
    producers::{precommit, testing},
};
use serde_json::{Value, json};

fn check(channel: &str, precommit: &str, testing: &str, expected: &str) {
    let fixture = match channel {
        "file" => Fixture::new().file(json!({
            "precommit_results":precommit,"local_testing_gate":testing})),
        "canonical" => Fixture::new()
            .env("RECIPE_VAR_precommit_results", precommit)
            .env("RECIPE_VAR_local_testing_gate", testing),
        "legacy" => Fixture::new()
            .env("PRECOMMIT_RESULTS", precommit)
            .env("LOCAL_TESTING_GATE", testing),
        _ => unreachable!(),
    };
    let result = fixture.run(&body(
        "workflow-precommit-test",
        "verification-terminal-evidence",
    ));
    result.exit(0);
    assert_eq!(
        result.json()["verification_completed"],
        expected,
        "{channel}"
    );
    assert_eq!(result.json()["terminal_no_op"], "false");
}

#[test]
fn single_structured_report_strings_preserve_all_transport_channels() {
    for channel in ["file", "canonical", "legacy"] {
        check(
            channel,
            &precommit().to_string(),
            &testing().to_string(),
            "true",
        );
    }
}

#[test]
fn multiple_documents_and_malformed_reports_cannot_upgrade_failure() {
    for channel in ["file", "canonical", "legacy"] {
        for corrupt in ["", "{", "null", "[]", "true", "\"PASS\""] {
            check(channel, corrupt, &testing().to_string(), "false");
            check(channel, &precommit().to_string(), corrupt, "false");
        }
        for (positive, other, is_precommit) in [
            (precommit(), testing(), true),
            (testing(), precommit(), false),
        ] {
            let mut failed = positive.clone();
            failed["status"] = json!("FAIL");
            failed["exit_code"] = json!(101);
            for corrupt in [
                format!("{failed}\n{positive}"),
                format!("{positive}\n{positive}"),
                format!("{positive}\nnull"),
            ] {
                if is_precommit {
                    check(channel, &corrupt, &other.to_string(), "false");
                } else {
                    check(channel, &other.to_string(), &corrupt, "false");
                }
            }
        }
    }
}

#[test]
fn nul_in_either_authoritative_report_fails_before_shell_capture() {
    for key in ["precommit_results", "local_testing_gate"] {
        let mut context = json!({"precommit_results":precommit().to_string(),
            "local_testing_gate":testing().to_string()});
        let report = context[key].as_str().unwrap().to_owned();
        context[key] = Value::String(format!("{report}\0"));
        let result = Fixture::new().file(context).run(&body(
            "workflow-precommit-test",
            "verification-terminal-evidence",
        ));
        result.exit(0);
        assert_eq!(result.json()["verification_completed"], "false", "{key}");
        assert!(
            result
                .stderr
                .contains("invalid authoritative workflow context")
        );
    }
}
