//! TDD (RED) contract tests for the additive `TurnOutput::exit_code` API —
//! PR-4 of issue #910.
//!
//! Written **first**: these FAIL to compile until PR-4 extends `TurnOutput`
//! with a private `exit_code: Option<i32>` field plus a `with_exit_code(code)`
//! builder and an `exit_code()` accessor. The extension MUST be additive and
//! backward-compatible:
//!
//!   * `from_text(..)` keeps its existing signature and yields `exit_code == None`.
//!   * `text()` is unchanged — the captured body is surfaced verbatim.
//!   * `with_exit_code(n)` is a `self -> Self` builder (last-write-wins).
//!   * `exit_code()` returns `Option<i32>` — `None` unless a code was attached.
//!
//! The auto-mode channel driver (also PR-4) branches on `exit_code()` to decide
//! phase transitions and terminal status, so `None` must be representable and
//! must never panic a consumer that only calls `from_text`.
//!
//! Run: `cargo test -p amplihack-turn --test turn_output_exit_code_it`.

use amplihack_turn::TurnOutput;

#[test]
fn from_text_yields_no_exit_code() {
    // A plain response body carries no subprocess exit code: the driver treats
    // `None` as "not a shelled-out turn" and must not fabricate a 0.
    let out = TurnOutput::from_text("agent said this");
    assert_eq!(
        out.exit_code(),
        None,
        "from_text must leave exit_code unset (None), not default to Some(0)"
    );
}

#[test]
fn from_text_preserves_text_verbatim() {
    // The additive field must not disturb the verbatim-text contract.
    let out = TurnOutput::from_text("line1\nline2\n");
    assert_eq!(out.text(), "line1\nline2\n");
}

#[test]
fn with_exit_code_sets_the_code_and_keeps_text() {
    let out = TurnOutput::from_text("ran to completion").with_exit_code(0);
    assert_eq!(
        out.exit_code(),
        Some(0),
        "with_exit_code(0) must attach Some(0) (distinct from the None default)"
    );
    assert_eq!(
        out.text(),
        "ran to completion",
        "attaching an exit code must not alter the captured text"
    );
}

#[test]
fn with_exit_code_carries_non_zero_codes() {
    let out = TurnOutput::from_text("boom").with_exit_code(2);
    assert_eq!(out.exit_code(), Some(2));
}

#[test]
fn with_exit_code_is_last_write_wins() {
    // The builder is chainable and the final call wins — the runner sets the
    // real subprocess code, so a later override must fully replace an earlier.
    let out = TurnOutput::from_text("x")
        .with_exit_code(1)
        .with_exit_code(3);
    assert_eq!(
        out.exit_code(),
        Some(3),
        "with_exit_code must be last-write-wins, not additive/ignored"
    );
}

#[test]
fn with_exit_code_accepts_negative_codes() {
    // Exit codes are `i32` (signal-derived codes can be negative on some
    // platforms); the field must round-trip any i32 without saturation.
    let out = TurnOutput::from_text("").with_exit_code(-1);
    assert_eq!(out.exit_code(), Some(-1));
}

#[test]
fn turn_output_stays_clone_and_debug() {
    // Downstream (the auto-mode channel) clones/logs outputs; the additive field
    // must not drop the existing `Clone`/`Debug` derives.
    let out = TurnOutput::from_text("keep derives").with_exit_code(7);
    let cloned = out.clone();
    assert_eq!(cloned.text(), "keep derives");
    assert_eq!(cloned.exit_code(), Some(7));
    let _ = format!("{out:?}");
}
