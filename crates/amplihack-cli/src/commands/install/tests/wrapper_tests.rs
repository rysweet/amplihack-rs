use super::helpers::create_exe_stub;
use super::*;

// ─── TDD: Group 4 — wrapper_matches type-directed idempotency ────────────

#[test]
fn wrapper_matches_returns_true_for_matching_binary_subcmd_wrapper() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let spec = HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
        async_rewake: false,
    };

    let wrapper = hooks::build_hook_wrapper(&spec, &hooks_bin);
    assert!(
        hooks::wrapper_matches(&wrapper, &spec, "amplihack"),
        "wrapper_matches must return true for an exact BinarySubcmd match"
    );
}

#[test]
fn wrapper_matches_returns_false_for_different_binary_subcmd() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let spec_session = HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
        async_rewake: false,
    };
    let spec_stop = HookSpec {
        event: "Stop",
        cmd: HookCommandKind::BinarySubcmd { subcmd: "stop" },
        timeout: Some(120),
        matcher: None,
        async_rewake: false,
    };

    let wrapper = hooks::build_hook_wrapper(&spec_session, &hooks_bin);
    assert!(
        !hooks::wrapper_matches(&wrapper, &spec_stop, "amplihack"),
        "wrapper_matches must reject wrapper with different subcmd"
    );
}

#[test]
fn wrapper_matches_returns_true_for_legacy_python_hook_wrapper() {
    let temp = tempfile::tempdir().unwrap();
    let _hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let spec = HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "workflow-classification-reminder",
        },
        timeout: Some(5),
        matcher: None,
        async_rewake: false,
    };

    let wrapper = serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py",
            "timeout": 5
        }]
    });
    assert!(
        hooks::wrapper_matches(&wrapper, &spec, "amplihack"),
        "wrapper_matches must return true for a legacy Python hook wrapper so install upgrades replace it in place"
    );
}

#[test]
fn merge_replaces_legacy_python_hook_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let mut settings = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py",
                    "timeout": 5
                }]
            }]
        }
    });

    let target = hook_block::HookTarget {
        bin: hooks_bin.clone(),
        limit_resume: true,
    };
    hook_block::merge(&mut settings, &target);

    let wrappers = settings["hooks"]["UserPromptSubmit"].as_array().unwrap();
    let commands: Vec<&str> = wrappers
        .iter()
        .filter_map(|w| w["hooks"][0]["command"].as_str())
        .collect();
    assert!(
        !commands
            .iter()
            .any(|c| c.contains("tools/amplihack/hooks/")),
        "legacy wrapper must be replaced, not kept: {commands:?}"
    );
    let reminders = commands
        .iter()
        .filter(|c| c.ends_with("workflow-classification-reminder"))
        .count();
    assert_eq!(reminders, 1, "exactly one native reminder: {commands:?}");
    assert!(commands[0].contains("amplihack-hooks"));
}

#[test]
fn wrapper_matches_recognizes_legacy_xpia_python_hook() {
    let spec = HookSpec {
        event: "PreToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
        async_rewake: false,
    };

    let wrapper = serde_json::json!({
        "matcher": "*",
        "hooks": [{
            "type": "command",
            "command": "/home/user/.amplihack/.claude/tools/xpia/hooks/pre_tool_use.py"
        }]
    });
    assert!(
        hooks::wrapper_matches(&wrapper, &spec, "xpia"),
        "wrapper_matches must return true for legacy XPIA Python hook wrappers so install upgrades replace them in place"
    );
}

#[test]
fn merge_removes_a_legacy_xpia_python_hook() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": "/home/user/.amplihack/.claude/tools/xpia/hooks/pre_tool_use.py"
                }]
            }]
        }
    });

    // Issue #2279 (D2): XPIA stays off; its wrapper is removed and only
    // amplihack's native wrapper is left.
    let target = hook_block::HookTarget {
        bin: hooks_bin.clone(),
        limit_resume: true,
    };
    hook_block::merge(&mut settings, &target);

    let wrappers = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(
        wrappers.len(),
        1,
        "legacy XPIA wrapper must be removed, not duplicated"
    );
    let command = wrappers[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(command.contains("amplihack-hooks"));
    assert!(command.ends_with("pre-tool-use"));
}
