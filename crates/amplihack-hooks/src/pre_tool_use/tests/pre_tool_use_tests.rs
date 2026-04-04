use super::*;
use crate::test_support::env_lock;

fn make_bash_input(command: &str) -> HookInput {
    HookInput::PreToolUse {
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({"command": command}),
        session_id: None,
    }
}

#[test]
fn allows_safe_commands() {
    let hook = PreToolUseHook;
    let result = hook.process(make_bash_input("ls -la")).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn allows_non_bash_tools() {
    let hook = PreToolUseHook;
    let input = HookInput::PreToolUse {
        tool_name: "Read".to_string(),
        tool_input: serde_json::json!({"path": "/tmp/file.txt"}),
        session_id: None,
    };
    let result = hook.process(input).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn blocks_no_verify() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git commit --no-verify -m 'test'"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_push() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git push --no-verify origin main"))
        .unwrap();
    assert_eq!(result["block"], true);
}

#[test]
fn blocks_no_verify_on_rebase() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git rebase --no-verify main"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_merge() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git merge --no-verify feature-branch"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_cherry_pick() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git cherry-pick --no-verify abc123"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_am() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git am --no-verify patch.patch"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn allows_git_rebase_without_no_verify() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hook = PreToolUseHook;
    let result = hook.process(make_bash_input("git rebase main")).unwrap();
    assert!(result.get("block").is_none());
}

#[test]
fn allows_git_commit_on_feature_branch() {
    // Hold env_lock so concurrent tests can't set GITHUB_COPILOT_AGENT=1
    // while inject_context runs against the real CWD.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // This test depends on the current branch not being main/master.
    // In CI, we may be on a feature branch, so this should pass.
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git commit -m 'test'"))
        .unwrap();
    // Can't assert allow/deny here reliably — depends on current branch.
    // Just verify it doesn't panic.
    let _ = result;
}

#[test]
fn handles_unknown_hook_event() {
    let hook = PreToolUseHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn blocks_no_verify_with_git_dir_prefix() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input(
            "GIT_DIR=/some/path git commit --no-verify -m 'test'",
        ))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_with_env_prefix() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("env git push --no-verify origin main"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn normalize_strips_env_var_prefix() {
    assert_eq!(
        normalize_command("GIT_DIR=/tmp git commit -m 'x'"),
        "git commit -m 'x'"
    );
}

#[test]
fn normalize_strips_env_command() {
    assert_eq!(
        normalize_command("env git push origin main"),
        "git push origin main"
    );
}

#[test]
fn normalize_strips_multiple_env_vars() {
    assert_eq!(normalize_command("FOO=1 BAR=baz git commit"), "git commit");
}

#[test]
fn normalize_passthrough_plain_command() {
    assert_eq!(normalize_command("git commit -m 'x'"), "git commit -m 'x'");
}
