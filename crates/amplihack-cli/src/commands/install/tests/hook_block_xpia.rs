//! Issue #2279 (D2, R9, AC6): XPIA stays off. No writer adds an XPIA hook,
//! an existing one is removed, and no writer adds, changes or removes an
//! `env` key: `XPIA_ENABLED` stays absent or `"false"` exactly as found.

use super::*;
use hook_block::HookTarget;
use serde_json::{Value, json};
use std::path::PathBuf;

const BIN: &str = "/opt/amplihack-test/bin/amplihack-hooks";

fn target() -> HookTarget {
    HookTarget {
        bin: PathBuf::from(BIN),
        limit_resume: true,
    }
}

fn xpia_wrapper(file: &str, matcher: Option<&str>) -> Value {
    let mut w = json!({
        "hooks": [{"type": "command", "command": format!("/opt/x/.claude/tools/xpia/hooks/{file}")}]
    });
    if let Some(m) = matcher {
        w["matcher"] = json!(m);
    }
    w
}

fn has_xpia(s: &Value) -> bool {
    s["hooks"].to_string().contains("tools/xpia/hooks/")
}

#[test]
fn merge_removes_existing_xpia_wrappers() {
    let mut s = json!({
        "hooks": {
            "SessionStart": [xpia_wrapper("session_start.py", None)],
            "PreToolUse": [xpia_wrapper("pre_tool_use.py", Some("*"))],
            "PostToolUse": [xpia_wrapper("post_tool_use.py", Some("*"))]
        }
    });
    hook_block::merge(&mut s, &target());
    assert!(!has_xpia(&s), "{}", s["hooks"]);
    assert!(hook_block::drift(&s, &target()).is_clean());
}

#[test]
fn is_xpia_command_checks_only_the_command_string() {
    assert!(hooks::is_xpia_command(
        "/opt/x/.claude/tools/xpia/hooks/pre_tool_use.py"
    ));
    assert!(!hooks::is_xpia_command("/opt/foreign/xpia-scanner"));
    assert!(!hooks::is_xpia_command(
        "\"/opt/b/amplihack-hooks\" pre-tool-use"
    ));
}

#[test]
fn xpia_enabled_absent_stays_absent() {
    let mut s = json!({"env": {"OTHER": "1"}});
    hook_block::merge(&mut s, &target());
    assert_eq!(s["env"], json!({"OTHER": "1"}));
}

#[test]
fn xpia_enabled_false_stays_false() {
    let mut s = json!({"env": {"XPIA_ENABLED": "false"}});
    hook_block::merge(&mut s, &target());
    assert_eq!(s["env"], json!({"XPIA_ENABLED": "false"}));
}

#[test]
fn no_env_key_is_created() {
    let mut s = json!({});
    hook_block::merge(&mut s, &target());
    assert!(s.get("env").is_none());
}

#[test]
fn register_hooks_has_no_xpia_parameter_and_writes_no_xpia_wrapper() {
    // D2: the with_xpia branch is gone from register_hooks.
    let mut s = json!({});
    settings::register_hooks(&mut s, std::path::Path::new(BIN), true);
    assert!(!has_xpia(&s));
}

#[cfg(unix)]
#[test]
fn ensure_settings_json_with_an_xpia_dir_writes_no_xpia_hook() {
    use std::fs;
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let staging = temp.path().join(".amplihack");
    fs::create_dir_all(staging.join(".claude/tools/xpia/hooks")).unwrap();
    let claude = temp.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    let settings_path = claude.join("settings.json");
    fs::write(&settings_path, r#"{"env":{"XPIA_ENABLED":"false"}}"#).unwrap();
    let hooks_bin = super::helpers::create_exe_stub(&temp.path().join("bin"), "amplihack-hooks");

    let result = settings::ensure_settings_json(&settings_path, &staging, 1, &hooks_bin, None);
    crate::test_support::restore_home(previous);
    result.unwrap();

    let s: Value = serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(!has_xpia(&s));
    assert_eq!(s["env"], json!({"XPIA_ENABLED": "false"}));
}
