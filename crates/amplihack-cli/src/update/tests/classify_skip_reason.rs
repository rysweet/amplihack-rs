use super::super::check::should_skip_update_check;
use super::super::check::{SkipReason, classify_skip_reason};
use super::super::*;
use super::SkipSignalEnvGuard;

#[test]
fn should_skip_update_check_when_ci_env_is_set_to_true() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    unsafe { std::env::set_var("CI", "true") };
    assert!(
        should_skip_update_check(&[OsString::from("amplihack"), OsString::from("copilot")]),
        "should_skip_update_check must return true for `amplihack copilot` when CI=true \
         (CI runner convention)"
    );
}

#[test]
fn should_skip_update_check_when_ci_env_is_set_to_one() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    unsafe { std::env::set_var("CI", "1") };
    assert!(
        should_skip_update_check(&[OsString::from("amplihack"), OsString::from("launch")]),
        "should_skip_update_check must return true for `amplihack launch` when CI=1 \
         (any non-empty value triggers skip)"
    );
}

#[test]
fn should_not_skip_update_check_when_ci_env_is_empty_string() {
    // Per design: CI is treated as a non-empty presence signal. An empty
    // string MUST NOT classify as SubprocessSafe.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    unsafe { std::env::set_var("CI", "") };
    assert!(
        !should_skip_update_check(&[OsString::from("amplihack"), OsString::from("copilot")]),
        "should_skip_update_check must NOT skip for `amplihack copilot` when CI is set \
         to the empty string — only non-empty values are subprocess-safe signals"
    );
}

#[test]
fn should_skip_update_check_when_agent_binary_env_is_set() {
    // AMPLIHACK_AGENT_BINARY=copilot signals that an outer agent binary
    // (e.g. Copilot CLI) is delegating into amplihack as a subprocess.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot") };
    assert!(
        should_skip_update_check(&[OsString::from("amplihack"), OsString::from("copilot")]),
        "should_skip_update_check must return true when AMPLIHACK_AGENT_BINARY is non-empty \
         (matches resolve_subprocess_safe semantics in commands/launch/command.rs)"
    );
}

#[test]
fn should_not_skip_update_check_when_agent_binary_env_is_empty_string() {
    // Empty AMPLIHACK_AGENT_BINARY is the documented sentinel for "no
    // delegation" (see resolve_subprocess_safe doc comment); it MUST NOT
    // classify as SubprocessSafe.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", "") };
    assert!(
        !should_skip_update_check(&[OsString::from("amplihack"), OsString::from("copilot")]),
        "empty AMPLIHACK_AGENT_BINARY is the 'no delegation' sentinel and must NOT \
         trigger subprocess-safe skip"
    );
}

#[test]
fn should_skip_update_check_when_subprocess_safe_flag_in_argv() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    assert!(
        should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("copilot"),
            OsString::from("--subprocess-safe"),
        ]),
        "should_skip_update_check must return true when argv contains the literal \
         token `--subprocess-safe`"
    );
}

#[test]
fn should_skip_update_check_when_subprocess_safe_flag_after_other_args() {
    // Linear scan: the flag may appear anywhere in args[1..], not only as
    // the immediate subcommand.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    assert!(
        should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("copilot"),
            OsString::from("--allow-all-tools"),
            OsString::from("--subprocess-safe"),
            OsString::from("--"),
            OsString::from("hello"),
        ]),
        "linear argv scan must find --subprocess-safe regardless of position"
    );
}

#[test]
fn should_not_skip_update_check_when_subprocess_safe_appears_as_substring() {
    // `--subprocess-safe-foo` or `--no-subprocess-safe` must NOT count.
    // Match must be by literal OsStr equality, not prefix/contains.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    assert!(
        !should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("copilot"),
            OsString::from("--subprocess-safe-foo"),
        ]),
        "literal-equality match: `--subprocess-safe-foo` must NOT trigger skip"
    );
    assert!(
        !should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("copilot"),
            OsString::from("--no-subprocess-safe"),
        ]),
        "literal-equality match: `--no-subprocess-safe` must NOT trigger skip"
    );
}

#[test]
fn should_skip_update_check_when_no_signals_set_for_non_launch_subcommand() {
    // Negative-control: with all skip-signal env vars cleared and a
    // non-launch subcommand, the existing NotLaunch path must still trigger
    // a skip (this is the only existing-behavior assertion in the new
    // suite — guards against accidental removal of the NotLaunch arm).
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    assert!(
        should_skip_update_check(&[OsString::from("amplihack"), OsString::from("update")]),
        "non-launch subcommand `update` must always skip the update check"
    );
}

#[test]
fn should_not_skip_update_check_when_no_signals_set_for_launch_subcommand() {
    // Negative-control: with EVERY skip-signal env var cleared and a
    // recognized launch subcommand, classification falls through to None
    // and the wrapper returns false. This is the test that, together with
    // the positive tests above, proves the new env-var/argv signals are
    // the only cause of the new skip behavior.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    assert!(
        !should_skip_update_check(&[OsString::from("amplihack"), OsString::from("copilot")]),
        "with all skip-signal env vars cleared, a launch subcommand must NOT skip"
    );
}

#[test]
fn classify_skip_reason_for_non_launch_subcommand_returns_not_launch_even_with_env_signals() {
    // Regression test for issue #625 outside-in finding: when stdin is a
    // TTY and the subcommand is non-launch (e.g. `amplihack --version`),
    // BUT a SubprocessSafe env signal is set (as is common inside agent
    // subprocesses where AMPLIHACK_AGENT_BINARY=copilot is exported), the
    // classification MUST resolve to NotLaunch, not SubprocessSafe.
    //
    // Per spec: "Do NOT emit for AMPLIHACK_NO_UPDATE_CHECK / AMPLIHACK_PARITY_TEST
    // or for non-launch subcommands." If SubprocessSafe took precedence over
    // NotLaunch, then `amplihack --version` running inside an agent subprocess
    // would unnecessarily emit the skip-line for a subcommand that never
    // would have triggered the update check in the first place.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();

    // Set every SubprocessSafe env signal simultaneously to prove that
    // NotLaunch wins over all of them when the subcommand is non-launch.
    unsafe {
        std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1");
        std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot");
        std::env::set_var("CI", "true");
    }

    for non_launch in ["--version", "--help", "version", "update", "doctor"] {
        let args = [OsString::from("amplihack"), OsString::from(non_launch)];
        let reason = classify_skip_reason(&args);
        assert_eq!(
            reason,
            Some(SkipReason::NotLaunch),
            "non-launch subcommand `{non_launch}` MUST classify as NotLaunch \
             (silent passthrough), not SubprocessSafe, even when every \
             SubprocessSafe env signal is set; got {reason:?}. \
             This protects `amplihack --version` from emitting the skip-line \
             when run inside an agent subprocess."
        );
    }
}

#[test]
fn classify_skip_reason_explicit_opt_out_wins_over_not_launch_for_non_launch_subcommand() {
    // ExplicitOptOut takes precedence over NotLaunch (both silent — order
    // is observable only via the SkipReason variant returned).
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = SkipSignalEnvGuard::capture_and_clear();
    unsafe { std::env::set_var(NO_UPDATE_CHECK_ENV, "1") };
    let args = [OsString::from("amplihack"), OsString::from("--version")];
    assert_eq!(
        classify_skip_reason(&args),
        Some(SkipReason::ExplicitOptOut),
        "ExplicitOptOut MUST take precedence over NotLaunch so the variant \
         accurately reflects user intent (preserved for future telemetry / \
         debug logging hooks)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #625: AMPLIHACK_TEST_FAKE_LATEST_VERSION network short-circuit.
//
// `fetch_latest_release` MUST honor the test-only env var: when set to a
// non-empty semver tag, return a synthetic `UpdateRelease` without any
// network call. Empty value MUST be ignored (production fall-through).
// ─────────────────────────────────────────────────────────────────────────────
