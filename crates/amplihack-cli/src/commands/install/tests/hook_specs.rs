use super::helpers::create_exe_stub;
use super::*;

// ─── TDD: Group 1 — HookCommandKind discriminant ──────────────────────────

#[test]
fn hook_command_kind_binary_subcmd_variant_exists() {
    let _kind = HookCommandKind::BinarySubcmd {
        subcmd: "session-start",
    };
}

#[test]
fn legacy_hook_script_name_maps_known_binary_subcommands() {
    assert_eq!(
        hooks::legacy_hook_script_name("workflow-classification-reminder"),
        Some("workflow_classification_reminder.py")
    );
    assert_eq!(
        hooks::legacy_hook_script_name("user-prompt-submit"),
        Some("user_prompt_submit.py")
    );
    assert_eq!(hooks::legacy_hook_script_name("unknown"), None);
}

// ─── TDD: Group 2 — AMPLIHACK_HOOK_SPECS canonical entries ───────────────

#[test]
fn amplihack_hook_specs_session_start_uses_binary_subcmd() {
    let spec = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| s.event == "SessionStart")
        .expect("SessionStart spec must exist");
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    assert_eq!(*subcmd, "session-start");
    assert_eq!(spec.timeout, Some(10));
    assert!(spec.matcher.is_none());
}

#[test]
fn amplihack_hook_specs_stop_uses_binary_subcmd() {
    let spec = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| s.event == "Stop")
        .expect("Stop spec must exist");
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    assert_eq!(*subcmd, "stop");
    assert_eq!(spec.timeout, Some(120));
    assert!(spec.matcher.is_none());
}

#[test]
fn amplihack_hook_specs_pre_tool_use_uses_binary_subcmd() {
    let spec = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| s.event == "PreToolUse")
        .expect("PreToolUse spec must exist");
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    assert_eq!(*subcmd, "pre-tool-use");
    assert!(
        spec.timeout.is_none(),
        "PreToolUse must omit timeout (fail-open)"
    );
    assert_eq!(spec.matcher, Some("*"));
}

#[test]
fn amplihack_hook_specs_post_tool_use_uses_binary_subcmd() {
    let spec = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| s.event == "PostToolUse")
        .expect("PostToolUse spec must exist");
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    assert_eq!(*subcmd, "post-tool-use");
    assert!(
        spec.timeout.is_none(),
        "PostToolUse must omit timeout (fail-open)"
    );
    assert_eq!(spec.matcher, Some("*"));
}

#[test]
fn amplihack_hook_specs_workflow_classification_uses_binary_subcmd() {
    let spec = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| {
            matches!(
                &s.cmd,
                HookCommandKind::BinarySubcmd { subcmd }
                    if *subcmd == "workflow-classification-reminder"
            )
        })
        .expect("workflow-classification-reminder BinarySubcmd spec must exist");
    assert_eq!(spec.event, "UserPromptSubmit");
    assert_eq!(spec.timeout, Some(5));
    assert!(spec.matcher.is_none());
}

#[test]
fn amplihack_hook_specs_user_prompt_submit_uses_binary_subcmd() {
    let specs: Vec<_> = AMPLIHACK_HOOK_SPECS
        .iter()
        .filter(|s| {
            s.event == "UserPromptSubmit"
                && matches!(
                    &s.cmd,
                    HookCommandKind::BinarySubcmd { subcmd } if *subcmd == "user-prompt-submit"
                )
        })
        .collect();
    assert_eq!(
        specs.len(),
        1,
        "Exactly one user-prompt-submit BinarySubcmd spec expected"
    );
    let HookCommandKind::BinarySubcmd { subcmd } = &specs[0].cmd;
    assert_eq!(*subcmd, "user-prompt-submit");
    assert_eq!(specs[0].timeout, Some(10));
}

#[test]
fn amplihack_hook_specs_pre_compact_uses_binary_subcmd() {
    let spec = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| s.event == "PreCompact")
        .expect("PreCompact spec must exist");
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    assert_eq!(*subcmd, "pre-compact");
    assert_eq!(spec.timeout, Some(30));
    assert!(spec.matcher.is_none());
}

#[test]
fn user_prompt_submit_workflow_classification_precedes_user_prompt_submit() {
    let python_pos = AMPLIHACK_HOOK_SPECS
        .iter()
        .position(|s| {
            matches!(
                &s.cmd,
                HookCommandKind::BinarySubcmd { subcmd }
                    if *subcmd == "workflow-classification-reminder"
            )
        })
        .expect("workflow-classification-reminder BinarySubcmd must exist");
    let binary_pos = AMPLIHACK_HOOK_SPECS
        .iter()
        .position(|s| {
            s.event == "UserPromptSubmit"
                && matches!(
                    &s.cmd,
                    HookCommandKind::BinarySubcmd { subcmd }
                        if *subcmd == "user-prompt-submit"
                )
        })
        .expect("BinarySubcmd user-prompt-submit must exist");
    assert!(
        python_pos < binary_pos,
        "workflow-classification-reminder (pos {python_pos}) must precede \
         user-prompt-submit (pos {binary_pos}) in AMPLIHACK_HOOK_SPECS"
    );
}

// ─── TDD: Group 3 — build_hook_wrapper generates correct command strings ──

#[test]
fn build_hook_wrapper_binary_subcmd_generates_binary_plus_subcmd() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let spec = HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
    };

    let wrapper = hooks::build_hook_wrapper(&spec, &hooks_bin);
    let hooks_arr = wrapper["hooks"]
        .as_array()
        .expect("wrapper must have hooks[]");
    let hook = hooks_arr[0].as_object().expect("hooks[0] must be object");

    let command = hook["command"].as_str().expect("hook must have command");
    assert!(
        command.contains("amplihack-hooks"),
        "command must reference the hooks binary, got: {command}"
    );
    assert!(
        command.ends_with("session-start"),
        "command must end with subcommand 'session-start', got: {command}"
    );

    let timeout = hook["timeout"].as_u64().expect("hook must have timeout");
    assert_eq!(timeout, 10);

    assert!(
        !wrapper.as_object().unwrap().contains_key("matcher"),
        "wrapper must not have matcher for SessionStart"
    );
}

#[test]
fn build_hook_wrapper_binary_subcmd_omits_timeout_when_none() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let spec = HookSpec {
        event: "PreToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    };

    let wrapper = hooks::build_hook_wrapper(&spec, &hooks_bin);
    let hooks_arr = wrapper["hooks"].as_array().unwrap();
    let hook = hooks_arr[0].as_object().unwrap();

    assert!(
        !hook.contains_key("timeout"),
        "PreToolUse hook must NOT include timeout field"
    );
    assert_eq!(
        wrapper["matcher"].as_str().unwrap(),
        "*",
        "wrapper must include matcher"
    );
}

#[test]
fn build_hook_wrapper_binary_subcmd_quotes_binary_path() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let spec = HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "workflow-classification-reminder",
        },
        timeout: Some(5),
        matcher: None,
    };

    let wrapper = hooks::build_hook_wrapper(&spec, &hooks_bin);
    let hooks_arr = wrapper["hooks"].as_array().unwrap();
    let hook = hooks_arr[0].as_object().unwrap();

    let command = hook["command"].as_str().expect("hook must have command");
    assert!(
        command.contains("amplihack-hooks"),
        "binary-subcommand command must reference the hooks binary, got: {command}"
    );
    assert!(
        command.ends_with("workflow-classification-reminder"),
        "binary-subcommand command must end with the Rust subcommand, got: {command}"
    );
    assert_eq!(hook["timeout"].as_u64().unwrap(), 5);
}
