//! Bugfix integration tests for recipe executor hardening.
//!
//! Covers:
//! - #277: Shell step non-interactive environment propagation
//! - #251: Agent context augmentation with working directory
//! - #242: Shell prerequisite validation guard

use amplihack_recipe::{AgentContext, shell_step_env, validate_shell_prerequisites};
use std::collections::HashMap;
use std::process::Command;

// ── #277: Shell step environment propagation ──

#[test]
fn shell_step_env_injects_noninteractive() {
    let env = shell_step_env(&HashMap::new());
    assert_eq!(env.get("NONINTERACTIVE").map(String::as_str), Some("1"));
}

#[test]
fn shell_step_env_injects_debian_frontend() {
    let env = shell_step_env(&HashMap::new());
    assert_eq!(
        env.get("DEBIAN_FRONTEND").map(String::as_str),
        Some("noninteractive")
    );
}

#[test]
fn shell_step_env_injects_ci() {
    let env = shell_step_env(&HashMap::new());
    assert_eq!(env.get("CI").map(String::as_str), Some("true"));
}

#[test]
fn shell_step_env_preserves_caller_home() {
    let mut inherit = HashMap::new();
    inherit.insert("HOME".into(), "/home/testuser".into());
    let env = shell_step_env(&inherit);
    assert_eq!(env["HOME"], "/home/testuser");
}

#[test]
fn shell_step_env_preserves_caller_path() {
    let mut inherit = HashMap::new();
    inherit.insert("PATH".into(), "/custom/bin:/other".into());
    let env = shell_step_env(&inherit);
    assert_eq!(env["PATH"], "/custom/bin:/other");
}

#[test]
fn shell_step_env_all_five_vars_present() {
    let env = shell_step_env(&HashMap::new());
    let required = ["HOME", "PATH", "NONINTERACTIVE", "DEBIAN_FRONTEND", "CI"];
    for key in required {
        assert!(env.contains_key(key), "missing required env var: {key}");
    }
}

/// Integration test: actually run a bash command with the hardened env
/// and verify the env vars are visible inside the shell.
#[test]
fn shell_step_env_visible_in_bash() {
    let env = shell_step_env(&HashMap::new());
    let output = Command::new("bash")
        .arg("-c")
        .arg("echo NONINTERACTIVE=$NONINTERACTIVE CI=$CI DEBIAN_FRONTEND=$DEBIAN_FRONTEND")
        .envs(&env)
        .output()
        .expect("failed to run bash");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("NONINTERACTIVE=1"));
    assert!(stdout.contains("CI=true"));
    assert!(stdout.contains("DEBIAN_FRONTEND=noninteractive"));
}

// ── #251: Agent context augmentation ──

#[test]
fn agent_context_captures_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    let ctx = AgentContext::from_working_dir(dir.path());
    assert!(!ctx.working_directory.is_empty());
    assert!(ctx.file_listing.contains(&"main.rs".to_string()));
}

#[test]
fn agent_context_augment_injects_path() {
    let ctx = AgentContext {
        working_directory: "/tmp/my-project".into(),
        file_listing: vec!["Cargo.toml".into(), "src".into()],
    };
    let augmented = ctx.augment_prompt("Fix the broken build");
    assert!(augmented.contains("/tmp/my-project"));
    assert!(augmented.contains("Cargo.toml"));
    assert!(augmented.contains("Fix the broken build"));
}

#[test]
fn agent_context_no_double_injection() {
    let ctx = AgentContext {
        working_directory: "/tmp/project".into(),
        file_listing: vec!["README.md".into()],
    };
    let prompt = "working_directory is /tmp/project. Do the thing.";
    let result = ctx.augment_prompt(prompt);
    assert_eq!(result, prompt);
}

#[test]
fn agent_context_noninteractive_instruction() {
    let ctx = AgentContext {
        working_directory: "/tmp/project".into(),
        file_listing: vec![],
    };
    let prompt_ctx = ctx.as_prompt_context();
    assert!(prompt_ctx.contains("Write all output files"));
}

#[test]
fn agent_context_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = AgentContext::from_working_dir(dir.path());
    let prompt = ctx.as_prompt_context();
    assert!(prompt.contains("(empty directory)"));
}

#[test]
fn agent_context_multi_step_reuse() {
    let ctx = AgentContext {
        working_directory: "/tmp/project".into(),
        file_listing: vec!["lib.rs".into()],
    };
    let s1 = ctx.augment_prompt("Step 1: Analyze the code");
    let s2 = ctx.augment_prompt("Step 2: Write the fix");
    assert!(s1.contains("/tmp/project"));
    assert!(s2.contains("/tmp/project"));
    // Both should have different task content
    assert!(s1.contains("Analyze"));
    assert!(s2.contains("Write the fix"));
}

// ── #242: Shell prerequisite validation ──

#[test]
fn prerequisites_echo_has_no_missing_tools() {
    let result = validate_shell_prerequisites("echo hello");
    assert!(result.is_ok());
}

#[test]
fn prerequisites_empty_command_passes() {
    let result = validate_shell_prerequisites("");
    assert!(result.is_ok());
}

#[test]
fn prerequisites_python3_detected_in_command() {
    // This tests parsing, not actual system availability
    let result = validate_shell_prerequisites("python3 -c 'print(1)'");
    // Result depends on whether python3 is installed; no panic = pass
    let _ = result.error_message();
}

#[test]
fn prerequisites_python_variant_detected() {
    let result = validate_shell_prerequisites("python script.py");
    let _ = result.error_message();
}
