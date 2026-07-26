//! R5 — teardown routing classifier (issue #1002).
//!
//! The bug: the hooks binary aliased `session-end` / `session-stop` to the
//! per-turn `StopHook`, and the per-turn `StopHook` performed Signal teardown
//! (`quitGroup`). That tears the group down after **every turn**, so the
//! whole-session channel dies immediately and the operator can only message the
//! very first turn.
//!
//! The fix separates two concerns with a single source of truth,
//! `amplihack_hooks::is_teardown_subcommand`:
//!   * whole-session teardown events  → SessionStop path (leave group once)
//!   * per-turn stop events           → StopHook (OUTBOUND relay only, no teardown)
//!
//! Wiring `sessionEnd` (Copilot) and `session-end`/`session-stop`/
//! `session-stop-event` to teardown — and keeping `stop`/`agentStop` per-turn —
//! is exactly what stops subscribers/groups from leaking on Copilot session end.
//!
//! RED: `is_teardown_subcommand` does not exist yet.
#![cfg(feature = "signal")]

use amplihack_hooks::is_teardown_subcommand;

#[test]
fn whole_session_end_events_are_teardown() {
    for name in [
        "session-end",
        "session-stop",
        "session-stop-event",
        "sessionEnd",
    ] {
        assert!(
            is_teardown_subcommand(name),
            "{name:?} must route to whole-session teardown"
        );
    }
}

#[test]
fn per_turn_stop_events_are_not_teardown() {
    // These fire once per assistant turn. They must NOT tear down the channel
    // (outbound relay only), otherwise the group is destroyed after turn 1.
    for name in ["stop", "agentStop"] {
        assert!(
            !is_teardown_subcommand(name),
            "{name:?} is a per-turn event and must NOT tear down the Signal channel"
        );
    }
}

#[test]
fn unrelated_subcommands_are_not_teardown() {
    for name in [
        "pre-tool-use",
        "post-tool-use",
        "user-prompt",
        "user-prompt-submit",
        "session-start",
        "pre-compact",
        "signal-subscriber",
        "",
    ] {
        assert!(
            !is_teardown_subcommand(name),
            "{name:?} must not be classified as session teardown"
        );
    }
}
