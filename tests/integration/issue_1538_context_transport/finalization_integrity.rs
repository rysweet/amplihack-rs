//! Malformed selected evidence must be detected as malformed.
//!
//! #2343 narrowed what that detection costs, not whether it happens. The
//! evidence record no longer names a terminal state of its own, and no blocker
//! rests on it alone, so unreadable evidence does not end the run. What it must
//! still do is read as unreadable rather than as a clean value, and say so in
//! the result. These cases assert that guarantee.
use super::assessed::{carry, recorded_outcome};
use super::fixtures::*;
use super::integrity_routes::{clean, fixture};
use serde_json::{Value, json};

const FIELDS: [&str; 5] = [
    "/git/dirty_worktree",
    "/tooling/missing",
    "/tooling/gh_required",
    "/prior_terminal_state/terminal_state",
    "/agent_outputs/hollow_success_signals",
];

/// Exit 1, no success, and no success token borrowed from the typed booleans.
fn assert_no_borrowed_success(f: Fixture) -> Value {
    let r = f.finalizer("validate");
    let j = r.json();
    assert_complete_schema(&j);
    r.exit(1);
    assert_eq!(j["terminal_success"], "false", "stderr={}", r.stderr);
    assert_eq!(j["terminal_failure"], "true");
    assert_eq!(j["finalizer_output_valid"], "false");
    for success in ["IMPLEMENTED_VERIFIED", "ALLOW_NO_OP", "EXISTING_OPEN_PR"] {
        assert_ne!(j["terminal_state"], success, "borrowed success: {j}");
    }
    j
}

/// Malformed evidence is detected as malformed: the result says the evidence
/// was unreadable, and no success is borrowed from it.
///
/// The `finalization_evidence=unreadable` check is what makes this assertion
/// bite. Since #2343 unreadable evidence no longer names a terminal state of
/// its own, so a run that merely reaches no decision proves nothing: a fixture
/// carrying no assessment at all ends the same way. What can still fail is the
/// detection itself, if a NUL-bearing, wrong-typed or unparseable field were
/// read as a clean value instead. Then the result would report no corruption,
/// and this would catch it.
fn assert_not_success(f: Fixture) {
    let j = assert_no_borrowed_success(f);
    let used = j["evidence_used"].as_str().unwrap();
    assert!(
        used.contains("finalization_evidence=unreadable"),
        "malformed evidence must be reported as unreadable, got {used}"
    );
}

/// The typed booleans a run would have to borrow a success from.
fn borrowing(f: Fixture) -> Fixture {
    f.env("IMPLEMENTATION_COMPLETED", "true")
        .env("VERIFICATION_COMPLETED", "true")
        .env("ALLOW_NO_OP", "true")
        .env("TERMINAL_NO_OP", "true")
}

fn assert_result(f: Fixture, state: &str, exit: i32) -> Value {
    let r = f.finalizer("validate");
    let j = r.json();
    assert_complete_schema(&j);
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
    // Well-formed evidence is read as such, never reported as unreadable.
    let used = j["evidence_used"].as_str().unwrap();
    assert!(!used.contains("finalization_evidence=unreadable"), "{used}");
    j
}

/// `evidence` with the outcome the collector records beside it (#2343).
fn with_outcome(mut evidence: Value) -> Value {
    evidence["outcome"] = recorded_outcome("");
    evidence
}

/// The result of `f` when `state` was decided over the transport it already
/// uses. The flat legacy aliases (route 4) cannot carry a decision: a run with
/// no `RECIPE_VAR_*` is on the legacy tier, where the assessment has no alias.
/// That route must end with no decision, which still proves its evidence was
/// read.
fn settled(f: Fixture, route: usize, state: &str, exit: i32) -> Value {
    match route {
        4 => assert_result(f, "FAILED_MISSING_TERMINAL_EVIDENCE", 1),
        _ => assert_result(carry(f, state), state, exit),
    }
}

fn malformed_fields(index: usize, values: &[Value]) {
    for value in values {
        for route in 0..5 {
            let mut evidence = clean();
            *evidence.pointer_mut(FIELDS[index]).unwrap() = value.clone();
            assert_not_success(fixture(evidence, route, false));
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
        assert_not_success(fixture(evidence, route, false));
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
                assert_not_success(fixture(evidence, route, false));
            }
        }
    }
}

#[test]
fn invalid_selected_transport_cannot_borrow_success() {
    for raw in ["{bad", "null", "[]", "{} {}", "\"{}\" \"{}\""] {
        // An env transport hands over a value that can be read and judged, so
        // a container that is not one JSON object is detected as corrupt and
        // the result says so.
        for f in [
            Fixture::new().env("RECIPE_VAR_finalization_evidence", raw),
            Fixture::new().env("FINALIZATION_EVIDENCE", raw),
        ] {
            assert_not_success(borrowing(f));
        }
        // A context file that is not one JSON object is absence, not
        // corruption (#2343): a transport that was lost or truncated must not
        // fail a run that finished its work, so there is nothing to report as
        // unreadable. What still holds is that no success is borrowed from the
        // typed booleans beside it.
        assert_no_borrowed_success(borrowing(Fixture::new().file_raw(raw)));
    }
}

#[test]
fn invalid_evidence_precedes_explicit_noop_and_later_hollow() {
    for route in 0..5 {
        let mut evidence = clean();
        evidence["git"]["dirty_worktree"] = json!("false\0");
        assert_not_success(fixture(evidence.clone(), route, true));
        evidence["agent_outputs"]["hollow_success_signals"] = json!(true);
        assert_not_success(fixture(evidence, route, false));
    }
}

// What a well-formed value in the evidence does not change: a clean, absent or
// null field, and a non-NUL control character in one, are read as evidence and
// not as corruption, so the decision arrives as made and the hollow-success
// signal beside the control character is still read intact.
#[test]
fn clean_absent_null_and_non_nul_controls_preserve_classification() {
    for route in 0..5 {
        for evidence in [clean(), json!({}), json!({"git":{"dirty_worktree":null}})] {
            let f = fixture(with_outcome(evidence), route, false);
            let j = settled(f, route, "IMPLEMENTED_VERIFIED", 0);
            assert_eq!(j["implementation_completed"], "true");
        }
        let j = settled(
            fixture(with_outcome(clean()), route, true),
            route,
            "ALLOW_NO_OP",
            0,
        );
        assert_eq!(j["terminal_no_op"], "true");
        for control in ["false\n", "false\t\r\u{1}", "\n"] {
            let mut evidence = with_outcome(clean());
            evidence["git"]["dirty_worktree"] = json!(control);
            let f = fixture(evidence.clone(), route, false);
            let j = settled(f, route, "IMPLEMENTED_VERIFIED", 0);
            assert_eq!(j["hollow_success_detected"], "false");
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
            let j = settled(f, route, "HOLLOW_SUCCESS", 1);
            assert_eq!(j["hollow_success_detected"], "true");
        }
    }
}
