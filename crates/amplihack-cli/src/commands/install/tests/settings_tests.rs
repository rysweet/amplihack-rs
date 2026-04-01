use super::*;
use super::helpers::*;
use std::fs;

// ─── TDD: Group 12 — ensure_settings_json returns (bool, Vec<String>) ────

#[test]
fn ensure_settings_json_returns_registered_event_names() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&staging_dir).unwrap();

    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_bin);
    }

    let result = settings::ensure_settings_json(&staging_dir, 99999, &hooks_bin);

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    crate::test_support::restore_home(previous);

    let (success, events) = result.expect("ensure_settings_json must not error");
    assert!(success, "must return true when hooks are present");
    assert!(!events.is_empty(), "must return non-empty event list");

    for expected in [
        "SessionStart",
        "Stop",
        "PreToolUse",
        "PostToolUse",
        "UserPromptSubmit",
        "PreCompact",
    ] {
        assert!(
            events.contains(&expected.to_string()),
            "events must include '{expected}', got: {events:?}"
        );
    }
}

#[test]
fn ensure_settings_json_succeeds_without_legacy_python_hook_files() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&staging_dir).unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let result = settings::ensure_settings_json(&staging_dir, 99999, &hooks_bin)
        .expect("settings setup should not depend on legacy python hook files");

    assert!(
        result.0,
        "settings setup must succeed with binary hook registrations"
    );
    assert!(temp.path().join(".claude/settings.json").exists());

    crate::test_support::restore_home(previous);
}

#[test]
fn ensure_settings_json_with_xpia_assets_keeps_unified_native_wrappers() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging_dir = temp.path().join(".amplihack/.claude");
    let xpia_hooks = staging_dir.join("tools/xpia/hooks");
    fs::create_dir_all(&xpia_hooks).unwrap();
    for file in XPIA_HOOK_FILES {
        fs::write(xpia_hooks.join(file), "print(1)\n").unwrap();
    }
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let result = settings::ensure_settings_json(&staging_dir, 99999, &hooks_bin)
        .expect("settings setup should succeed with staged xpia assets");
    assert!(result.0, "settings setup must succeed");

    let settings_raw = fs::read_to_string(temp.path().join(".claude/settings.json")).unwrap();
    let settings_val: serde_json::Value = serde_json::from_str(&settings_raw).unwrap();
    let pre_tool_use = settings_val["hooks"]["PreToolUse"].as_array().unwrap();

    assert_eq!(
        pre_tool_use.len(),
        1,
        "xpia staging should not duplicate native PreToolUse wrappers"
    );
    let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(
        command.contains("amplihack-hooks") && command.ends_with("pre-tool-use"),
        "xpia-enabled install must keep the unified native hook wrapper, got: {command}"
    );
    assert!(
        !command.contains("tools/xpia/"),
        "fresh native settings must not retain legacy xpia python paths: {command}"
    );

    crate::test_support::restore_home(previous);
}

#[test]
fn validate_amplihack_native_hook_contract_accepts_canonical_native_wrappers() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let settings_val = build_canonical_native_hook_settings(&hooks_bin);

    let drift = settings::validate_amplihack_native_hook_contract(&settings_val);

    assert!(
        drift.is_empty(),
        "canonical native hook settings should validate cleanly: {drift:?}"
    );
}

#[test]
fn validate_amplihack_native_hook_contract_reports_duplicate_native_drift() {
    let temp = tempfile::tempdir().unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let mut settings_val = build_canonical_native_hook_settings(&hooks_bin);

    let root = hooks::ensure_object(&mut settings_val);
    let hooks_map = hooks::ensure_object(
        root.entry("hooks")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
    );
    let wrappers = hooks::ensure_array(
        hooks_map
            .entry("UserPromptSubmit")
            .or_insert_with(|| serde_json::Value::Array(Vec::new())),
    );
    let drift_spec = HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "workflow-classification-reminder",
        },
        timeout: Some(5),
        matcher: None,
    };
    wrappers.push(hooks::build_hook_wrapper(&drift_spec, &hooks_bin));

    let drift = settings::validate_amplihack_native_hook_contract(&settings_val);

    assert!(
        drift
            .iter()
            .any(|issue| issue.contains("workflow-classification-reminder"))
            && drift
                .iter()
                .any(|issue| issue.contains("unexpected native hook")),
        "duplicate native drift must be surfaced explicitly, got: {drift:?}"
    );
}

#[test]
fn missing_framework_paths_does_not_require_legacy_python_hook_files() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    create_minimal_staged_assets(temp.path());

    let missing =
        settings::missing_framework_paths(&temp.path().join(".amplihack/.claude")).unwrap();

    assert!(
        missing.is_empty(),
        "legacy python hook files must not be treated as required staged assets: {missing:?}"
    );

    crate::test_support::restore_home(previous);
}

#[test]
fn backup_metadata_is_always_valid_json() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    fs::create_dir_all(temp.path().join(".claude")).unwrap();
    fs::write(temp.path().join(".claude/settings.json"), "{}").unwrap();

    let staging_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&staging_dir).unwrap();

    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let timestamp = 1_700_000_000_u64;
    let _ = settings::ensure_settings_json(&staging_dir, timestamp, &hooks_bin);

    crate::test_support::restore_home(previous);

    let metadata_path = staging_dir
        .join("runtime/sessions")
        .join(format!("install_{timestamp}_backup.json"));

    if metadata_path.exists() {
        let raw = fs::read_to_string(&metadata_path).unwrap();
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&raw);
        assert!(
            parsed.is_ok(),
            "backup metadata must be valid JSON, got:\n{raw}"
        );
        let meta = parsed.unwrap();
        assert!(
            meta.get("settings_path").is_some(),
            "backup metadata must have 'settings_path'"
        );
        assert!(
            meta.get("backup_path").is_some(),
            "backup metadata must have 'backup_path'"
        );
    }
}
