//! TDD: Failing tests for `amplihack_cli::env_builder::helpers::active_agent_binary`
//! after refactor. The helper must delegate to the shared resolver and:
//! 1. Default to "copilot" when no env, no launcher_context.
//! 2. Honor `AMPLIHACK_AGENT_BINARY` allowlisted override.
//! 3. NEVER return "claude" as the unset-env default.
//!
//! Each test spawns the current test binary as a child probe with a specific
//! env configuration, avoiding in-process env mutation races under the
//! parallel test harness.

#![allow(clippy::unwrap_used)]

use std::process::Command;

/// Spawn the current test binary to run a single child probe test in a
/// subprocess with full env isolation.  `env_override` sets
/// AMPLIHACK_AGENT_BINARY when `Some`; `None` removes it entirely.
fn run_probe(test_name: &str, env_override: Option<&str>) -> String {
    let exe = std::env::current_exe().expect("could not resolve current test exe");
    let mut cmd = Command::new(&exe);
    cmd.args(["--exact", test_name, "--nocapture"]);
    cmd.env_remove("AMPLIHACK_AGENT_BINARY");
    // These probes assert what the LOWER layers answer, so every layer above
    // the one under test has to be silent. Session markers are exported by
    // whichever CLI is running the suite, so a developer running `cargo test`
    // from inside a Claude Code session would otherwise see layer 2 answer
    // "claude" and these tests go red — while CI, which has no such session,
    // stayed green. Sourced from SESSION_MARKERS so adding a marker cannot
    // silently reopen that gap.
    for (key, _) in amplihack_utils::agent_binary::SESSION_MARKERS {
        cmd.env_remove(key);
    }
    if let Some(val) = env_override {
        cmd.env("AMPLIHACK_AGENT_BINARY", val);
    }
    let output = cmd.output().expect("failed to spawn child probe");
    // stdout contains test harness lines plus the printed value; find the
    // one line that is just the binary name (no spaces, not a harness line).
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with("running")
                && !t.starts_with("test ")
                && !t.starts_with("test result")
        })
        .unwrap_or("")
        .trim()
        .to_string()
}

// ── Child probe tests (run in subprocess by the parent tests below) ─────────
//
// These are real #[test] functions so the harness can invoke them with
// `--exact <name>`.  They call active_agent_binary() directly and print the
// result; the parent test reads stdout and asserts on it.

#[test]
fn probe_default_no_env() {
    let v = amplihack_cli::env_builder::helpers::active_agent_binary();
    println!("{v}");
}

#[test]
fn probe_claude_override() {
    let v = amplihack_cli::env_builder::helpers::active_agent_binary();
    println!("{v}");
}

#[test]
fn probe_invalid_override() {
    let v = amplihack_cli::env_builder::helpers::active_agent_binary();
    println!("{v}");
}

// ── Contract tests (each spawns a subprocess with isolated env) ──────────────

#[test]
fn default_is_copilot_not_claude() {
    let result = run_probe("probe_default_no_env", None);
    assert_eq!(result, "copilot", "default flipped from copilot to claude");
}

#[test]
fn explicit_claude_override_still_works() {
    let result = run_probe("probe_claude_override", Some("claude"));
    assert_eq!(result, "claude");
}

#[test]
fn rejected_override_falls_back_to_copilot() {
    let result = run_probe("probe_invalid_override", Some("not-a-real-binary"));
    assert_eq!(result, "copilot");
}

/// The regression this file exists to catch, stated as a test rather than as a
/// convention someone has to remember.
///
/// A session marker is a legitimate resolution layer (issue #1342): it names
/// the CLI actually hosting the process, so it must outrank a file on disk.
/// But it also means "nothing is set" is no longer the same as "no environment
/// variables about the agent binary" — and a probe that clears only
/// AMPLIHACK_AGENT_BINARY is testing the marker layer while claiming to test
/// the default.
///
/// This bit twice: once through a /proc ancestry layer that was removed for it,
/// and again through markers, which fail identically for a different reason. CI
/// cannot see either, because CI runs in no session at all.
#[test]
fn a_session_marker_answers_before_the_default_and_probes_must_clear_it() {
    let markers = amplihack_utils::agent_binary::SESSION_MARKERS;
    assert!(
        markers.iter().any(|(_, b)| *b == "claude") && markers.iter().any(|(_, b)| *b == "copilot"),
        "both vendors must be representable, or the layer is a one-way voter"
    );

    // Every marker the resolver consults must be one the probe clears. Reading
    // the probe's own source keeps the two from drifting apart silently.
    let probe_src = include_str!("active_agent_binary_default_test.rs");
    assert!(
        probe_src.contains("for (key, _) in amplihack_utils::agent_binary::SESSION_MARKERS"),
        "run_probe must clear the marker list from its single source of truth, \
         not from a hand-copied list that can fall behind"
    );
}
