//! TDD: Failing tests for `amplihack_cli::env_builder::helpers::active_agent_binary`
//! after refactor. The helper must delegate to the shared resolver and:
//! 1. Default to "copilot" when no env, no launcher_context.
//! 2. Honor `AMPLIHACK_AGENT_BINARY` allowlisted override.
//! 3. NEVER return "claude" as the unset-env default.

#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use amplihack_cli::env_builder::helpers::active_agent_binary;

// Serialize env-var tests: std::env::set_var/remove_var are process-global and
// Rust's test harness runs tests in parallel by default.  Without this guard
// the clear_env() call in one test races with set_env() in another, producing
// flaky results in CI.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn clear_env() {
    unsafe {
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    }
}
fn set_env(v: &str) {
    unsafe {
        std::env::set_var("AMPLIHACK_AGENT_BINARY", v);
    }
}

#[test]
fn default_is_copilot_not_claude() {
    let _guard = ENV_MUTEX.lock().unwrap();
    clear_env();
    let v = active_agent_binary();
    assert_eq!(v, "copilot", "default flipped from claude to copilot");
}

#[test]
fn explicit_claude_override_still_works() {
    let _guard = ENV_MUTEX.lock().unwrap();
    set_env("claude");
    let result = active_agent_binary();
    clear_env();
    assert_eq!(result, "claude");
}

#[test]
fn rejected_override_falls_back_to_copilot() {
    let _guard = ENV_MUTEX.lock().unwrap();
    set_env("not-a-real-binary");
    let result = active_agent_binary();
    clear_env();
    assert_eq!(result, "copilot");
}
