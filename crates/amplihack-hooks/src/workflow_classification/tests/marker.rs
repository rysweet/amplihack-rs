//! The marker rule, the state file and the marked and unmarked paths through
//! `process` (issue #1742). The work-request rule is in `work_request`.

use super::support::*;
use super::*;

// ---- constants (D1) ----

#[test]
fn coordination_prefix_is_the_documented_marker() {
    assert_eq!(COORDINATION_PREFIX, "[coord]");
    assert_eq!(
        COORDINATION_PREFIX.len(),
        7,
        "D2 compares exactly seven bytes"
    );
}

#[test]
fn coordination_note_has_the_exact_documented_text() {
    assert_eq!(COORDINATION_NOTE, EXPECTED_NOTE);
}

#[test]
fn coordination_note_contains_no_launch_instructions() {
    for banned in LAUNCH_STRINGS {
        assert!(
            !COORDINATION_NOTE.contains(banned),
            "COORDINATION_NOTE must not contain {banned:?}"
        );
    }
    assert!(
        !COORDINATION_NOTE.contains("any recipe"),
        "the note must not forbid the workflow outright: a code change still goes through it"
    );
    assert!(
        COORDINATION_NOTE.contains("If it asks for a code change, use dev-orchestrator as usual."),
        "a marked code request that the work-request rule misses must still reach \
         default-workflow (routing_prompt.txt, downstream invariant)"
    );
    assert!(
        !COORDINATION_NOTE.contains('<'),
        "the note is plain text, not a system-reminder wrapper"
    );
}

// ---- marker rule (D2, D3) ----

#[test]
fn marker_matching_rules() {
    let cases: &[(&str, Option<&str>)] = &[
        ("", None),
        ("   ", None),
        ("[coo", None),
        ("[coord", None),
        ("[coörd] x", None),
        ("[coor€] x", None), // byte 7 falls inside a multibyte char: no panic
        ("€€€ [coord] x", None),
        ("[coordination] status", None),
        ("please [coord] status", None),
        ("coord status", None),
        ("(coord) status", None),
        (
            "[coord] report your lane status",
            Some("report your lane status"),
        ),
        ("  [COORD] status?", Some("status?")),
        ("\t\n[Coord]   x", Some("x")),
        ("[coord]status", Some("status")),
        ("[coord]", Some("")),
        ("[coord]   ", Some("")),
        ("[coord] build #1740", Some("build #1740")),
        ("[coord] a [coord] b", Some("a [coord] b")),
    ];
    for (input, expected) in cases {
        assert_eq!(
            strip_coordination_prefix(input),
            *expected,
            "strip_coordination_prefix({input:?})"
        );
    }
}

// ---- state (requirement 4) ----

#[test]
fn old_state_file_deserialises() {
    let state: ClassificationState =
        serde_json::from_str(r#"{"last_classified_turn":2,"session_id":"s"}"#)
            .expect("a state file written before #1742 still loads");
    assert_eq!(state.last_classified_turn, Some(2));
    assert_eq!(state.session_id, "s");
    assert_eq!(state.last_rule, None);
    assert_eq!(state.observed_turns, 0);
}

/// A session with no classification has no `last_classified_turn`: it is
/// neither read as turn 0 nor written as one (review round 3 of PR #1752).
#[test]
fn an_unclassified_session_has_no_classified_turn() {
    let state: ClassificationState =
        serde_json::from_str(r#"{"session_id":"s","observed_turns":2}"#).unwrap();
    assert_eq!(state.last_classified_turn, None);
    let back = serde_json::to_value(&state).unwrap();
    assert!(
        back.get("last_classified_turn").is_none(),
        "an absent turn is not written as 0 or null: {back}"
    );
    let state: ClassificationState =
        serde_json::from_str(r#"{"last_classified_turn":null,"session_id":"s"}"#).unwrap();
    assert_eq!(state.last_classified_turn, None);
}

#[test]
fn last_rule_round_trips() {
    let state: ClassificationState = serde_json::from_str(
        r#"{"last_classified_turn":1,"session_id":"s","observed_turns":4,"last_rule":"throttled"}"#,
    )
    .unwrap();
    assert_eq!(state.last_rule.as_deref(), Some("throttled"));
    let back = serde_json::to_value(&state).unwrap();
    assert_eq!(back["last_rule"], "throttled");
    assert_eq!(back["observed_turns"], 4);
}

// ---- process(): marked paths (D5) ----

#[test]
fn coord_status_gets_note_not_router() {
    let iso = Isolated::new();
    let out = run_hook(Some("s-note"), Some(0), "[coord] report your lane status");
    assert_note(&out);

    let state = iso.read_state("s-note").expect("the outcome is recorded");
    assert_eq!(last_rule(&state), Some("coordination_marker"));
}

#[test]
fn unmarked_status_gets_router_with_calibration() {
    let iso = Isolated::new();
    let out = run_hook(Some("s-plain"), Some(5), "report your lane status");
    assert_router(&out);
    let text = out["additionalContext"].as_str().unwrap();
    assert!(
        text.contains("\"report your lane status\" = Q&A"),
        "the unmarked message is calibrated as Q&A in the routing prompt"
    );

    let state = iso.read_state("s-plain").unwrap();
    assert_eq!(last_rule(&state), Some("classification"));
    assert_eq!(classified_turn(&state), Some(5));
}

/// A marked work request is throttled like the same text without the marker
/// (review round 6 of PR #1752). Inside the window it gets the note, which
/// tells the agent to use dev-orchestrator for a code change, where the
/// unmarked text gets nothing. Outside it, it is routed.
#[test]
fn coord_explicit_request_is_throttled_like_the_unmarked_text() {
    let iso = Isolated::new();
    iso.seed_classified("s-build", 10);

    let out = run_hook(Some("s-build"), Some(11), "[coord] build #1740");
    assert_note(&out);
    let state = iso.read_state("s-build").unwrap();
    assert_eq!(last_rule(&state), Some("coordination_throttled"));
    assert_eq!(classified_turn(&state), Some(10), "nothing was classified");

    let out = run_hook(Some("s-build"), Some(14), "[coord] build #1740");
    assert_router(&out);
    let state = iso.read_state("s-build").unwrap();
    assert_eq!(last_rule(&state), Some("coordination_explicit_dev"));
    assert_eq!(classified_turn(&state), Some(14));
}

#[test]
fn the_same_request_unmarked_is_still_throttled() {
    let iso = Isolated::new();
    iso.seed_classified("s-build-plain", 10);

    let out = run_hook(Some("s-build-plain"), Some(11), "build #1740");
    assert_eq!(out, empty());

    let state = iso.read_state("s-build-plain").unwrap();
    assert_eq!(last_rule(&state), Some("throttled"));
    assert_eq!(classified_turn(&state), Some(10));
}

#[test]
fn coord_dev_command_is_silent() {
    let iso = Isolated::new();
    iso.seed_classified("s-dev", 3);

    let out = run_hook(Some("s-dev"), Some(30), "[coord] /dev fix the parser");
    assert_eq!(out, empty());

    let state = iso.read_state("s-dev").unwrap();
    assert_eq!(last_rule(&state), Some("explicit_dev_command"));
    assert_eq!(
        classified_turn(&state),
        Some(3),
        "no classification took place"
    );
}

/// Marked code requests worded the way a coordinator would word them reach
/// the routing prompt on a new topic, and through it dev-orchestrator and
/// default-workflow. Before round 1 of the #1752 review these got the note,
/// which told the agent to skip every recipe and change the code directly.
/// Turn 14 is outside the window of the classification at turn 10.
#[test]
fn coord_change_requests_are_routed() {
    let iso = Isolated::new();
    let prompts = [
        "[coord] fix the parser for #12",
        "[coord] refactor #57",
        "[coord] update PR 57",
        "[coord] implement retry in installer.rs",
        "[coord] add a retry to the installer",
        "[coord] build 1740",
    ];
    for (i, prompt) in prompts.iter().enumerate() {
        let session = format!("s-change-{i}");
        iso.seed_classified(&session, 10);
        let out = run_hook(Some(&session), Some(14), prompt);
        assert_router(&out);
        let state = iso.read_state(&session).unwrap();
        assert_eq!(
            last_rule(&state),
            Some("coordination_explicit_dev"),
            "prompt {prompt:?}"
        );
        assert_eq!(classified_turn(&state), Some(14), "prompt {prompt:?}");
    }
}

/// Everything else gets the note. A code request that the rule still misses,
/// such as `please fix the parser`, is covered by the note's own wording: it
/// tells the agent to use dev-orchestrator for a code change.
#[test]
fn coord_non_request_gets_note() {
    let iso = Isolated::new();
    let prompts = [
        "[coord] please fix the parser",
        "[coord] build failed on PR 12",
        "[coord] build is green on #1740",
        "[coord] update on PR 57: merged",
        "[coord] fix landed on PR 12",
        "[coord] building #1740",
        "[coord] fixes #3",
        "[coord] update: CI passed on PR 12",
        "[coord] CI passed on PR 1739, nothing for you to do",
        "[coord] handing PR 1741 over to you",
        "[coord]status",
        "  [COORD] status?",
        "[coord]",
        "[coord] what's the status of #12",
    ];
    for (i, prompt) in prompts.iter().enumerate() {
        let session = format!("s-nonreq-{i}");
        let out = run_hook(Some(&session), Some(0), prompt);
        assert_note(&out);
        let state = iso.read_state(&session).unwrap();
        assert_eq!(
            last_rule(&state),
            Some("coordination_marker"),
            "prompt {prompt:?}"
        );
    }
}

#[test]
fn note_is_emitted_on_every_turn() {
    let _iso = Isolated::new();
    for turn in [0u64, 1, 2, 3, 4, 50] {
        let out = run_hook(Some("s-every"), Some(turn), "[coord] status?");
        assert_note(&out);
    }
}

/// The note does not count as a classification. Turn 15 is outside the
/// window of the classification at turn 10, and would be inside the window of
/// one at turn 14, so the probe fails if the note at turn 14 is recorded as a
/// classification.
#[test]
fn note_does_not_advance_classified_turn() {
    let iso = Isolated::new();
    iso.seed_classified("s-keep", 10);

    assert_note(&run_hook(Some("s-keep"), Some(14), "[coord] status?"));

    let state = iso.read_state("s-keep").unwrap();
    assert_eq!(classified_turn(&state), Some(10));
    assert_eq!(last_rule(&state), Some("coordination_marker"));

    let out = run_hook(
        Some("s-keep"),
        Some(15),
        "Please investigate the install flow",
    );
    assert_router(&out);
    let state = iso.read_state("s-keep").unwrap();
    assert_eq!(classified_turn(&state), Some(15));
    assert_eq!(last_rule(&state), Some("classification"));
}

/// Review round 3 of PR #1752, item 2. With no turn count, as live sessions
/// send, two notes in a fresh session used to leave a default
/// `last_classified_turn` of 0 that throttled the first real prompt at turn 2.
#[test]
fn notes_in_a_fresh_session_do_not_throttle_the_next_prompt() {
    let iso = Isolated::new();
    assert_note(&run_hook(Some("s-fresh"), None, "[coord] status?"));
    assert_note(&run_hook(
        Some("s-fresh"),
        None,
        "[coord] handing PR 57 over",
    ));

    let state = iso.read_state("s-fresh").unwrap();
    assert_eq!(state["observed_turns"], 2);
    assert_eq!(
        classified_turn(&state),
        None,
        "nothing has been classified: {state}"
    );

    let out = run_hook(Some("s-fresh"), None, "Please investigate the install flow");
    assert_router(&out);
    let state = iso.read_state("s-fresh").unwrap();
    assert_eq!(state["observed_turns"], 3);
    assert_eq!(classified_turn(&state), Some(2));
    assert_eq!(last_rule(&state), Some("classification"));

    // The throttle applies from the first real classification on.
    assert_eq!(
        run_hook(Some("s-fresh"), None, "Please investigate that too"),
        empty()
    );
}

/// The same holds for unmarked turns that classify nothing, such as explicit
/// `/dev` commands: before review round 3 a prompt at turn 2 or 3 was
/// throttled against a classification at turn 0 that never took place.
#[test]
fn unclassified_dev_commands_do_not_throttle_the_next_prompt() {
    let iso = Isolated::new();
    assert_eq!(
        run_hook(Some("s-dev2"), None, "/dev fix the parser"),
        empty()
    );
    assert_eq!(run_hook(Some("s-dev2"), None, "/dev add a test"), empty());
    assert_router(&run_hook(
        Some("s-dev2"),
        None,
        "Please investigate the install flow",
    ));
    let state = iso.read_state("s-dev2").unwrap();
    assert_eq!(classified_turn(&state), Some(2));
}

#[test]
fn misplaced_or_misspelt_markers_are_classified_as_usual() {
    let iso = Isolated::new();
    for (i, prompt) in ["please [coord] status", "[coordination] status"]
        .iter()
        .enumerate()
    {
        let session = format!("s-miss-{i}");
        let out = run_hook(Some(&session), Some(0), prompt);
        assert_router(&out);
        let state = iso.read_state(&session).unwrap();
        assert_eq!(last_rule(&state), Some("classification"), "{prompt:?}");
    }
}

// ---- process(): unmarked paths (D5) ----

#[test]
fn last_rule_on_unmarked_paths() {
    let iso = Isolated::new();

    // Explicit /dev: silent, no classification recorded.
    iso.seed_classified("s-u-dev", 1);
    assert_eq!(run_hook(Some("s-u-dev"), Some(9), "/dev fix this"), empty());
    let state = iso.read_state("s-u-dev").unwrap();
    assert_eq!(last_rule(&state), Some("explicit_dev_command"));
    assert_eq!(classified_turn(&state), Some(1));

    // New topic: routed and recorded.
    assert_router(&run_hook(
        Some("s-u-new"),
        Some(0),
        "Please investigate this",
    ));
    let state = iso.read_state("s-u-new").unwrap();
    assert_eq!(last_rule(&state), Some("classification"));
    assert_eq!(classified_turn(&state), Some(0));

    // Throttled: silent, classified turn unchanged.
    assert_eq!(
        run_hook(Some("s-u-new"), Some(2), "Please investigate that"),
        empty()
    );
    let state = iso.read_state("s-u-new").unwrap();
    assert_eq!(last_rule(&state), Some("throttled"));
    assert_eq!(classified_turn(&state), Some(0));
}

#[test]
fn recording_an_outcome_keeps_observed_turns() {
    let iso = Isolated::new();
    // No turnCount: the hook counts prompts itself.
    assert_note(&run_hook(Some("s-obs"), None, "[coord] one"));
    assert_note(&run_hook(Some("s-obs"), None, "[coord] two"));
    assert_note(&run_hook(Some("s-obs"), None, "[coord] three"));

    let state = iso.read_state("s-obs").unwrap();
    assert_eq!(state["observed_turns"], 3);
    assert_eq!(state["session_id"], "s-obs");
    assert_eq!(last_rule(&state), Some("coordination_marker"));
}
