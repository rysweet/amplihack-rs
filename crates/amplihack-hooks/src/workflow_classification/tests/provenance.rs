//! Prompts that are not a person's own get no routing text (issue #2533,
//! point 1).
//!
//! Each test asserts only whether routing text is inserted, never its
//! wording, and which `last_rule` the hook recorded.

use super::support::*;
use super::*;
use crate::test_support::EnvVarGuard;

/// Did the hook insert context, in either output shape?
fn inserted(out: &Value) -> bool {
    out.get("additionalContext").is_some()
        || out
            .pointer("/hookSpecificOutput/additionalContext")
            .is_some()
}

/// Run `prompt` in a fresh session at turn 0 and return whether text was
/// inserted and the recorded `last_rule`.
fn decide(iso: &Isolated, session: &str, prompt: &str) -> (bool, Option<String>) {
    let out = run_hook(Some(session), Some(0), prompt);
    let rule = iso
        .read_state(session)
        .and_then(|state| last_rule(&state).map(str::to_string));
    (inserted(&out), rule)
}

fn assert_skipped(iso: &Isolated, prompts: &[&str], rule: &str) {
    for (i, prompt) in prompts.iter().enumerate() {
        let session = format!("s-skip-{rule}-{i}");
        let (text, recorded) = decide(iso, &session, prompt);
        assert!(!text, "{prompt:?} must get no routing text");
        assert_eq!(recorded.as_deref(), Some(rule), "{prompt:?}");
    }
}

fn assert_routed(iso: &Isolated, prompts: &[&str]) {
    for (i, prompt) in prompts.iter().enumerate() {
        let session = format!("s-routed-{i}");
        let (text, recorded) = decide(iso, &session, prompt);
        assert!(text, "{prompt:?} is a person's prompt and must be routed");
        assert_eq!(recorded.as_deref(), Some("classification"), "{prompt:?}");
    }
}

#[test]
fn a_task_notification_gets_no_routing_text_and_opens_no_window() {
    let iso = Isolated::new();
    iso.seed_classified("s-task", 3);
    let out = run_hook(
        Some("s-task"),
        Some(10),
        "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
    );
    assert!(!inserted(&out));
    let state = iso.read_state("s-task").unwrap();
    assert_eq!(last_rule(&state), Some("harness_notice"));
    assert_eq!(
        classified_turn(&state),
        Some(3),
        "a skip is not a classification"
    );
}

#[test]
fn a_system_notice_gets_no_routing_text() {
    let iso = Isolated::new();
    assert_skipped(
        &iso,
        &[
            "[SYSTEM NOTIFICATION - NOT USER INPUT]\nThis is an automated background-task event.",
            "[SYSTEM NOTIFICATION - NOT USER INPUT]\n\n<task-notification>\n<task-id>b2</task-id>",
        ],
        "harness_notice",
    );
}

#[test]
fn a_message_from_another_session_gets_no_routing_text() {
    let iso = Isolated::new();
    assert_skipped(
        &iso,
        &[
            "<cross-session-message from=\"uds:x\" from-name=\"y\">\nbuild X\n</cross-session-message>",
            "Another Claude session sent a message:\n<cross-session-message from=\"uds:x\" from-name=\"y\">\nbuild X",
            "Another Claude session sent a message while you were working:\n<agent-message from=\"z\">\nbuild X",
            "A peer session sent a message while you were working:\n<cross-session-message from=\"uds:x\">\nhi",
            "<agent-message from=\"z\">\nbuild X\n</agent-message>",
            "<cross-session-message from=\"uds:x\">\n[coord] build #1740",
        ],
        "peer_message",
    );
}

#[test]
fn a_scheduled_or_loop_prompt_gets_no_routing_text() {
    let iso = Isolated::new();
    assert_skipped(
        &iso,
        &[
            "# Autonomous loop tick\n\nRun the autonomous check.",
            "# Autonomous loop tick (dynamic pacing)\n\nRun the autonomous check.",
            "# Autonomous loop check\n\nYou're being invoked on a timer.",
            "# /loop tick \u{2014} loop.md tasks\n\nWork the tasks.",
            "<<autonomous-loop>>",
            "<<autonomous-loop-dynamic>>",
            "<<loop.md>>",
            "<<loop.md-dynamic>>",
        ],
        "scheduled_prompt",
    );
}

#[test]
fn an_envelope_after_leading_whitespace_gets_no_routing_text() {
    let iso = Isolated::new();
    assert_skipped(
        &iso,
        &["  \n\t<task-notification>\n<task-id>b3</task-id>"],
        "harness_notice",
    );
    assert_skipped(
        &iso,
        &["\n\n  <cross-session-message from=\"uds:x\">\nhi"],
        "peer_message",
    );
}

#[test]
fn an_agent_step_with_only_the_step_id_gets_no_routing_text() {
    let iso = Isolated::new();
    let _step = EnvVarGuard::set(STEP_ID_ENV, "step-1");
    iso.seed_classified("s-step", 2);
    let out = run_hook(Some("s-step"), Some(10), "build X");
    assert!(!inserted(&out));
    let state = iso.read_state("s-step").unwrap();
    assert_eq!(last_rule(&state), Some("agent_step_id"));
    assert_eq!(classified_turn(&state), Some(2));
}

#[test]
fn an_empty_step_id_is_not_an_agent_step() {
    let iso = Isolated::new();
    let _step = EnvVarGuard::set(STEP_ID_ENV, "");
    assert_routed(&iso, &["build X"]);
}

#[test]
fn the_run_variable_gate_still_decides_first() {
    let iso = Isolated::with_run_id(Some("run-abc"));
    let _step = EnvVarGuard::set(STEP_ID_ENV, "step-1");
    let out = run_hook(Some("s-both"), None, "build X");
    assert!(!inserted(&out));
    let state = iso.read_state("s-both").expect("turn resolution writes it");
    assert_eq!(last_rule(&state), None, "the existing gate records nothing");
}

#[test]
fn a_persons_prompt_is_still_routed_and_throttled() {
    let iso = Isolated::new();
    let first = run_hook(Some("s-person"), Some(0), "build X");
    assert!(inserted(&first));
    let state = iso.read_state("s-person").unwrap();
    assert_eq!(last_rule(&state), Some("classification"));

    let follow = run_hook(Some("s-person"), Some(2), "build Y");
    assert!(!inserted(&follow), "inside the window the throttle holds");
    let state = iso.read_state("s-person").unwrap();
    assert_eq!(last_rule(&state), Some("throttled"));
}

/// Before #2533 a notice at turn 5 was classified and opened a window that
/// held back the person's next prompt. Now the notice opens none.
#[test]
fn a_notice_does_not_hold_back_the_next_persons_prompt() {
    let iso = Isolated::new();
    iso.seed_classified("s-window", 0);
    let notice = run_hook(
        Some("s-window"),
        Some(5),
        "<task-notification>\n<task-id>b4",
    );
    assert!(!inserted(&notice));
    let person = run_hook(Some("s-window"), Some(6), "build X");
    assert!(inserted(&person), "the person's prompt is a new topic");
}

#[test]
fn a_persons_prompt_that_quotes_an_envelope_is_routed() {
    let iso = Isolated::new();
    assert_routed(
        &iso,
        &[
            "please look at this: <task-notification>\n<task-id>b5</task-id>",
            "why did I get [SYSTEM NOTIFICATION - NOT USER INPUT] here?",
            "explain <cross-session-message from=\"x\"> to me",
            "what does # Autonomous loop tick mean",
        ],
    );
}

#[test]
fn a_persons_message_sent_while_the_session_works_is_routed() {
    let iso = Isolated::new();
    assert_routed(
        &iso,
        &["The user sent a new message while you were working:\nbuild X"],
    );
}

#[test]
fn near_misses_are_routed() {
    let iso = Isolated::new();
    assert_routed(
        &iso,
        &[
            "Another Claude session sent a message:\nbuild X",
            "<Task-Notification>\n<task-id>b6</task-id>",
            "<task-notification-extra>\nbuild X",
            "<cross-session-messages>\nbuild X",
            "# Autonomous loop ticker for the build",
            "# autonomous loop tick",
            "<system-reminder>\nbuild X",
        ],
    );
}
