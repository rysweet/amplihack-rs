//! Included as a child of limit_resume_decide to reuse its finite fake I/O.
//! The Deps publication method resolves trusted attribution internally and
//! publishes before the existing resume gates.
use super::*;

#[test]
fn an_actual_limit_is_published_even_when_auto_switch_is_disabled() {
    let mut deps = Fake::off();
    assert_eq!(
        run_with(&limit_input("s", "limit"), &mut deps),
        Outcome::Disabled
    );
    assert_eq!(
        deps.publications.len(),
        1,
        "publication must precede the resume gate"
    );
    assert!(deps.waits.is_empty());
    assert!(deps.keys.is_empty());
}

#[test]
fn a_down_rotator_does_not_prevent_publishing_limit_evidence() {
    let mut deps = Fake::new();
    deps.rotator_down = true;
    assert_eq!(
        run_with(&limit_input("s", "limit"), &mut deps),
        Outcome::RotatorDown
    );
    assert_eq!(deps.publications.len(), 1);
    assert!(deps.waits.is_empty());
}

#[test]
fn stop_hook_active_still_publishes_but_does_not_bypass_resume_protection() {
    let mut deps = Fake::new();
    let event = input(
        json!({"hook_event_name":"StopFailure", "session_id":"s", "error":"rate_limit", "stop_hook_active":true}),
    );
    assert_eq!(run_with(&event, &mut deps), Outcome::StopHookActive);
    assert_eq!(deps.publications.len(), 1);
    assert!(deps.waits.is_empty());
    assert!(deps.keys.is_empty());
}

#[test]
fn unrelated_stops_and_invalid_sessions_do_not_publish_limit_evidence() {
    for event in [
        input(json!({"hook_event_name":"StopFailure", "session_id":"s", "error":"api_error"})),
        input(json!({"hook_event_name":"StopFailure", "error":"rate_limit"})),
        HookInput::Unknown,
    ] {
        let mut deps = Fake::new();
        assert_eq!(run_with(&event, &mut deps), Outcome::NotLimit);
        assert!(deps.publications.is_empty());
    }
}

#[test]
fn publication_refusal_does_not_weaken_the_existing_resume_gate() {
    let mut deps = Fake::off();
    deps.publication_error = true;
    assert_eq!(
        run_with(&limit_input("s", "limit"), &mut deps),
        Outcome::Disabled
    );
    assert!(deps.publications.is_empty());
    assert!(deps.waits.is_empty());
    assert!(deps.keys.is_empty());
}
