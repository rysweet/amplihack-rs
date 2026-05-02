//! TDD: Failing tests for `amplihack_cli::env_builder::helpers::active_agent_binary`
//! after refactor. The helper must delegate to the shared resolver and:
//! 1. Default to "copilot" when no env, no launcher_context.
//! 2. Honor `AMPLIHACK_AGENT_BINARY` allowlisted override.
//! 3. NEVER return "claude" as the unset-env default.

#![allow(clippy::unwrap_used)]

use amplihack_cli::env_builder::helpers::active_agent_binary;
use std::sync::Mutex;

// Tests in this file mutate the shared process env var `AMPLIHACK_AGENT_BINARY`,
// so they must be serialized to avoid races when cargo test runs them in parallel.
static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    let v = active_agent_binary();
    assert_eq!(v, "copilot", "default flipped from claude to copilot");
}

#[test]
fn explicit_claude_override_still_works() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_env("claude");
    assert_eq!(active_agent_binary(), "claude");
    clear_env();
}

#[test]
fn rejected_override_falls_back_to_copilot() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_env("not-a-real-binary");
    assert_eq!(active_agent_binary(), "copilot");
    clear_env();
}
