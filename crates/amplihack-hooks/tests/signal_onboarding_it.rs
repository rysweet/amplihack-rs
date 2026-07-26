//! R1 — onboarding prompt gating + declined sentinel (issue #1002).
//!
//! When a host has no Signal config yet, amplihack should *ask* (once) whether
//! to link this device and mirror the session. The prompt must be:
//!   * skippable and non-blocking (it never runs the QR/device-link flow inside
//!     the ~30s hook budget — it only records intent / spawns setup detached),
//!   * suppressed in non-interactive mode (`AMPLIHACK_NONINTERACTIVE=1`),
//!   * suppressed when there is no TTY,
//!   * suppressed forever after the user declines (a persisted sentinel),
//!   * suppressed once Signal is already configured.
//!
//! The decision is factored into a pure function `should_prompt(&OnboardingEnv)`
//! so every gate combination is deterministically testable without touching a
//! real terminal, and the declined sentinel round-trips through an explicit
//! `root` (hermetic temp dir — no HOME/cwd mutation).
//!
//! RED: the `onboarding` module and its items do not exist yet.
#![cfg(feature = "signal")]

use amplihack_hooks::signal_integration::onboarding::{
    OnboardingDecision, OnboardingEnv, mark_onboarding_declined, onboarding_declined,
    onboarding_declined_path, should_prompt,
};

/// The only state in which we prompt: unconfigured + interactive TTY + not
/// previously declined.
fn ripe() -> OnboardingEnv {
    OnboardingEnv {
        config_present: false,
        is_tty: true,
        noninteractive: false,
        declined_before: false,
    }
}

#[test]
fn prompts_only_when_unconfigured_interactive_and_not_declined() {
    assert_eq!(should_prompt(&ripe()), OnboardingDecision::Prompt);
}

#[test]
fn skips_when_signal_already_configured() {
    let env = OnboardingEnv {
        config_present: true,
        ..ripe()
    };
    assert_eq!(
        should_prompt(&env),
        OnboardingDecision::Skip,
        "already configured ⇒ never prompt"
    );
}

#[test]
fn skips_in_non_interactive_mode() {
    // Respect AMPLIHACK_NONINTERACTIVE=1: automation must never block on a prompt.
    let env = OnboardingEnv {
        noninteractive: true,
        ..ripe()
    };
    assert_eq!(should_prompt(&env), OnboardingDecision::Skip);
}

#[test]
fn skips_when_no_tty() {
    // No controlling terminal ⇒ nobody to answer ⇒ skip (never break the session).
    let env = OnboardingEnv {
        is_tty: false,
        ..ripe()
    };
    assert_eq!(should_prompt(&env), OnboardingDecision::Skip);
}

#[test]
fn skips_after_previously_declined() {
    let env = OnboardingEnv {
        declined_before: true,
        ..ripe()
    };
    assert_eq!(
        should_prompt(&env),
        OnboardingDecision::Skip,
        "a prior decline must suppress re-prompting on every launch"
    );
}

#[test]
fn declined_sentinel_round_trips_under_explicit_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    assert!(
        !onboarding_declined(root),
        "fresh root has no declined sentinel"
    );
    mark_onboarding_declined(root).expect("write sentinel");
    assert!(
        onboarding_declined(root),
        "after marking declined, the sentinel must be observed"
    );
    assert!(
        onboarding_declined_path(root).starts_with(root),
        "sentinel path must live under the provided root"
    );
}

#[test]
fn marking_declined_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mark_onboarding_declined(root).unwrap();
    mark_onboarding_declined(root).unwrap(); // second call must not error
    assert!(onboarding_declined(root));
}
