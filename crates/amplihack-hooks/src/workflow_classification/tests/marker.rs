//! The marker rule, the work-request rule and the marked and unmarked paths
//! through `process` (issue #1742).

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

// ---- explicit work request rule ----

/// A marked body that starts with a change verb is a request for work and is
/// routed. Issue #1742: "an explicit development request inside a
/// coordination message still routes ... because it names the work". A
/// coordinator words such requests naturally, so the rule does not ask for an
/// issue or PR number straight after the verb.
#[test]
fn explicit_work_request_rule() {
    let matches = [
        "build #1740",
        "BUILD #1740",
        "fix issue 12",
        "fix issue #12",
        "fix issue#12",
        "work on PR #57",
        "work   on pr 57",
        "implement pr 3 now",
        "develop #9",
        "start issue 4",
        "Start #4 when ready",
        // Requests that name the work without a number next to the verb.
        "fix the parser for #12",
        "Fix\tthe parser",
        "refactor #57",
        "update PR 57",
        "implement retry in installer.rs",
        "add a retry to the installer",
        "create docs/reference/retry.md",
        "write tests for the parser",
        "build 1740",
        "start the build for issue 5",
        // A report that starts with a verb is routed too. The routing prompt
        // then classifies it, and its rule 8 treats a result report as Q&A.
        "fix landed on PR 12",
    ];
    for body in matches {
        assert!(
            is_explicit_work_request(body),
            "{body:?} starts with a change verb and must be routed"
        );
    }

    let non_matches = [
        "",
        "build",
        "fix   ",
        "building #1740",
        "fixes #3",
        "rebuild #1740",
        "fix-up landed on main",
        "update: CI passed on PR 12",
        "work #5",
        "please build #1740",
        "report your lane status",
        "CI passed on PR 12",
        "handing PR 57 over to you",
        "PR 134 is merged, retry the install",
        "what's the status of #12",
    ];
    for body in non_matches {
        assert!(
            !is_explicit_work_request(body),
            "{body:?} does not start with a change verb and a following word"
        );
    }
}

/// The verb list must cover every verb of the routing prompt's IMPLEMENT
/// signal, so a marked body that the routing prompt would call DEV is routed
/// to it and not given the note.
#[test]
fn work_request_verbs_cover_the_implement_signal() {
    let start = DEFAULT_ROUTING_PROMPT
        .find("IMPLEMENT<br/>")
        .expect("the routing prompt has an IMPLEMENT signal")
        + "IMPLEMENT<br/>".len();
    let end = start
        + DEFAULT_ROUTING_PROMPT[start..]
            .find("\"]")
            .expect("the IMPLEMENT node is closed");
    let verbs: Vec<&str> = DEFAULT_ROUTING_PROMPT[start..end]
        .split(|c: char| c == ',' || c.is_whitespace() || c == '<' || c == '>' || c == '/')
        .filter(|word| !word.is_empty() && *word != "br")
        .collect();
    assert_eq!(
        verbs,
        [
            "build", "fix", "add", "create", "refactor", "update", "write"
        ],
        "the IMPLEMENT signal changed; update EXPLICIT_WORK_REQUEST_PATTERN with it"
    );
    for verb in verbs {
        let body = format!("{verb} the retry logic");
        assert!(
            is_explicit_work_request(&body),
            "IMPLEMENT verb {verb:?} is missing from the work-request rule"
        );
    }
}

// ---- state (requirement 4) ----

#[test]
fn old_state_file_deserialises() {
    let state: ClassificationState =
        serde_json::from_str(r#"{"last_classified_turn":2,"session_id":"s"}"#)
            .expect("a state file written before #1742 still loads");
    assert_eq!(state.last_classified_turn, 2);
    assert_eq!(state.session_id, "s");
    assert_eq!(state.last_rule, None);
    assert_eq!(state.observed_turns, 0);
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

#[test]
fn coord_explicit_request_ignores_throttle() {
    let iso = Isolated::new();
    iso.seed_classified("s-build", 10);

    let out = run_hook(Some("s-build"), Some(11), "[coord] build #1740");
    assert_router(&out);

    let state = iso.read_state("s-build").unwrap();
    assert_eq!(last_rule(&state), Some("coordination_explicit_dev"));
    assert_eq!(classified_turn(&state), Some(11));
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
/// the routing prompt, and through it dev-orchestrator and default-workflow.
/// Before round 1 of the #1752 review these got the note, which told the agent
/// to skip every recipe and change the code directly.
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
        let out = run_hook(Some(&session), Some(11), prompt);
        assert_router(&out);
        let state = iso.read_state(&session).unwrap();
        assert_eq!(
            last_rule(&state),
            Some("coordination_explicit_dev"),
            "prompt {prompt:?}"
        );
        assert_eq!(classified_turn(&state), Some(11), "prompt {prompt:?}");
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

#[test]
fn note_does_not_advance_classified_turn() {
    let iso = Isolated::new();
    iso.seed_classified("s-keep", 2);

    assert_note(&run_hook(Some("s-keep"), Some(20), "[coord] status?"));

    let state = iso.read_state("s-keep").unwrap();
    assert_eq!(classified_turn(&state), Some(2));
    assert_eq!(last_rule(&state), Some("coordination_marker"));

    // The next human prompt is judged as if the note had never happened.
    let out = run_hook(
        Some("s-keep"),
        Some(21),
        "Please investigate the install flow",
    );
    assert_router(&out);
    let state = iso.read_state("s-keep").unwrap();
    assert_eq!(classified_turn(&state), Some(21));
    assert_eq!(last_rule(&state), Some("classification"));
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
