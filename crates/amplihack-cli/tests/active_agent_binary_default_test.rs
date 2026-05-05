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
