//! Malformed selected evidence must fail before positive completion or no-op.
use super::fixtures::*;
use serde_json::{Value, json};

const FIELDS: [&str; 5] = [
    "/git/dirty_worktree",
    "/tooling/missing",
    "/tooling/gh_required",
    "/prior_terminal_state/terminal_state",
    "/agent_outputs/hollow_success_signals",
];
const OUTPUT: [&str; 19] = [
    "terminal_success",
    "terminal_state",
    "terminal_reason",
    "required_next_action",
    "hollow_success_detected",
    "evidence_used",
    "finalizer_schema_version",
    "finalizer_confidence",
    "finalizer_output_valid",
    "reporting_failure",
    "implementation_completed",
    "verification_completed",
    "publish_state_reached",
    "terminal_no_op",
    "terminal_failure",
    "pr_url",
    "pr_number",
    "observed_phases",
    "missing_evidence",
];

fn clean() -> Value {
    json!({"git":{"dirty_worktree":false},"tooling":{"missing":"","gh_required":false},
        "prior_terminal_state":{"terminal_state":""},
        "agent_outputs":{"hollow_success_signals":false}})
}

fn selected(evidence: Value, route: usize, noop: bool) -> Fixture {
    let implementation = json!({"implementation_completed":!noop,"terminal_no_op":noop});
    let verification = json!({"verification_completed":!noop});
    let f = Fixture::new()
        .env("FINALIZATION_EVIDENCE", clean().to_string())
        .env("FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE", "false")
        .env("FINALIZATION_EVIDENCE_TOOLING_MISSING", "")
        .env("FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED", "false")
        .env(
            "FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE",
            "",
        )
        .env(
            "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
            "false",
        )
        .env(
            "RECIPE_VAR_finalization_evidence__git__dirty_worktree",
            "false",
        )
        .env("IMPLEMENTATION_COMPLETED", "true")
        .env("VERIFICATION_COMPLETED", "true")
        .env("ALLOW_NO_OP", "true")
        .env("TERMINAL_NO_OP", "true");
    match route {
        0 | 1 => f
            .file(json!({"finalization_evidence":if route == 0 {evidence} else {json!(evidence.to_string())},
                "implementation_terminal_evidence":implementation,
                "verification_terminal_evidence":verification,"allow_no_op":noop}))
            .env("RECIPE_VAR_finalization_evidence", clean().to_string()),
        2 | 3 => f
            .env("RECIPE_VAR_finalization_evidence", if route == 2 {
                evidence.to_string()
            } else {
                json!(evidence.to_string()).to_string()
            })
            .env("RECIPE_VAR_implementation_terminal_evidence", implementation.to_string())
            .env("RECIPE_VAR_verification_terminal_evidence", verification.to_string())
            .env("RECIPE_VAR_allow_no_op", noop.to_string()),
        _ => unreachable!(),
    }
}

fn fixture(evidence: Value, route: usize, noop: bool) -> Fixture {
    if route == 4 {
        return Fixture::new()
            .env("FINALIZATION_EVIDENCE", evidence.to_string())
            .env("FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE", "false")
            .env("FINALIZATION_EVIDENCE_TOOLING_MISSING", "")
            .env("FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED", "false")
            .env(
                "FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE",
                "",
            )
            .env(
                "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
                "false",
            )
            .env(
                "IMPLEMENTATION_COMPLETED",
                if noop { "false" } else { "true" },
            )
            .env(
                "VERIFICATION_COMPLETED",
                if noop { "false" } else { "true" },
            )
            .env("TERMINAL_NO_OP", "true")
            .env("ALLOW_NO_OP", "true");
    }
    selected(evidence, route, noop)
}

fn assert_result(f: Fixture, state: &str, exit: i32) {
    let r = f.finalizer("validate");
    let j = r.json();
    let object = j.as_object().unwrap();
    assert_eq!(object.len(), OUTPUT.len(), "complete schema: {j}");
    for field in OUTPUT {
        assert!(
            object.get(field).is_some_and(Value::is_string),
            "String {field}: {j}"
        );
    }
    assert_eq!(
        j["terminal_state"], state,
        "child exit={} stderr={}",
        r.code, r.stderr
    );
    r.exit(exit);
    assert_eq!(
        j["terminal_success"],
        if exit == 0 { "true" } else { "false" }
    );
    assert_eq!(
        j["terminal_failure"],
        if exit == 0 { "false" } else { "true" }
    );
    assert_eq!(
        j["finalizer_output_valid"],
        if exit == 0 { "true" } else { "false" }
    );
}

fn malformed_fields(index: usize, values: &[Value]) {
    for value in values {
        for route in 0..5 {
            let mut evidence = clean();
            *evidence.pointer_mut(FIELDS[index]).unwrap() = value.clone();
            assert_result(
                fixture(evidence, route, false),
                "FAILED_INVALID_EVIDENCE",
                1,
            );
        }
    }
}

macro_rules! field_cases {
    ($nul:ident, $types:ident, $index:expr) => {
        #[test]
        fn $nul() {
            malformed_fields(
                $index,
                &[json!("\0false"), json!("fal\0se"), json!("false\0")],
            );
        }
        #[test]
        fn $types() {
            malformed_fields($index, &[json!(0), json!([]), json!({})]);
        }
    };
}
field_cases!(nul_dirty, wrong_type_dirty, 0);
field_cases!(nul_missing, wrong_type_missing, 1);
field_cases!(nul_gh, wrong_type_gh, 2);
field_cases!(nul_prior, wrong_type_prior, 3);
field_cases!(nul_hollow, wrong_type_hollow, 4);

fn retained(route: usize) {
    for two_fields in [true, false] {
        let mut evidence = clean();
        evidence["git"]["dirty_worktree"] = json!("false\0");
        if two_fields {
            evidence["agent_outputs"]["hollow_success_signals"] = json!("true\0");
        }
        assert_result(
            fixture(evidence, route, false),
            "FAILED_INVALID_EVIDENCE",
            1,
        );
    }
}
#[test]
fn retained_file_object() {
    retained(0);
}
#[test]
fn retained_file_string() {
    retained(1);
}
#[test]
fn retained_canonical_object() {
    retained(2);
}
#[test]
fn retained_canonical_string() {
    retained(3);
}
#[test]
fn retained_legacy_object() {
    retained(4);
}

#[test]
fn malformed_containers_cannot_borrow_clean_aliases() {
    for key in ["git", "tooling", "prior_terminal_state", "agent_outputs"] {
        for value in [json!(false), json!("bad"), json!([]), json!(1)] {
            for route in 0..5 {
                let mut evidence = clean();
                evidence[key] = value.clone();
                assert_result(
                    fixture(evidence, route, false),
                    "FAILED_INVALID_EVIDENCE",
                    1,
                );
            }
        }
    }
}

#[test]
fn invalid_selected_transport_cannot_borrow_success() {
    for raw in ["{bad", "null", "[]", "{} {}", "\"{}\" \"{}\""] {
        for f in [
            Fixture::new().file_raw(raw),
            Fixture::new().env("RECIPE_VAR_finalization_evidence", raw),
            Fixture::new().env("FINALIZATION_EVIDENCE", raw),
        ] {
            assert_result(
                f.env("IMPLEMENTATION_COMPLETED", "true")
                    .env("VERIFICATION_COMPLETED", "true")
                    .env("ALLOW_NO_OP", "true")
                    .env("TERMINAL_NO_OP", "true"),
                "FAILED_INVALID_EVIDENCE",
                1,
            );
        }
    }
}

#[test]
fn invalid_evidence_precedes_explicit_noop_and_later_hollow() {
    for route in 0..5 {
        let mut evidence = clean();
        evidence["git"]["dirty_worktree"] = json!("false\0");
        assert_result(
            fixture(evidence.clone(), route, true),
            "FAILED_INVALID_EVIDENCE",
            1,
        );
        evidence["agent_outputs"]["hollow_success_signals"] = json!(true);
        assert_result(
            fixture(evidence, route, false),
            "FAILED_INVALID_EVIDENCE",
            1,
        );
    }
}

#[test]
fn clean_absent_null_and_non_nul_controls_preserve_classification() {
    for route in 0..5 {
        for evidence in [clean(), json!({}), json!({"git":{"dirty_worktree":null}})] {
            assert_result(fixture(evidence, route, false), "IMPLEMENTED_VERIFIED", 0);
        }
        assert_result(fixture(clean(), route, true), "ALLOW_NO_OP", 0);
        for control in ["false\n", "false\t\r\u{1}", "\n"] {
            let mut evidence = clean();
            evidence["git"]["dirty_worktree"] = json!(control);
            assert_result(
                fixture(evidence.clone(), route, false),
                "IMPLEMENTED_VERIFIED",
                0,
            );
            evidence["agent_outputs"]["hollow_success_signals"] = json!(true);
            let f = fixture(evidence, route, false);
            let f = if route == 4 {
                f.env(
                    "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
                    "true",
                )
            } else {
                f
            };
            assert_result(f, "HOLLOW_SUCCESS", 1);
        }
    }
}
