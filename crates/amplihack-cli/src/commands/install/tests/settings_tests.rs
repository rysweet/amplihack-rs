use super::helpers::*;
use super::*;
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
fn ensure_settings_json_with_xpia_dir_keeps_unified_native_wrappers() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging_dir = temp.path().join(".amplihack/.claude");
    let xpia_hooks = staging_dir.join("tools/xpia/hooks");
    fs::create_dir_all(&xpia_hooks).unwrap();
    // No .py or .sh stub files — the Rust binary IS the XPIA implementation.
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let result = settings::ensure_settings_json(&staging_dir, 99999, &hooks_bin)
        .expect("settings setup should succeed with xpia dir present (no script files needed)");
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

// ─── TDD: XPIA Python removal — tests written before implementation ─────────

#[test]
fn xpia_hook_files_constant_must_not_exist() {
    // After the change, XPIA_HOOK_FILES should be deleted from types.rs.
    // This test verifies the constant is not referenced anywhere in production
    // code by checking the source file directly.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let types_src = fs::read_to_string(
        std::path::Path::new(manifest_dir).join("src/commands/install/types.rs"),
    )
    .expect("types.rs must be readable");

    assert!(
        !types_src.contains("XPIA_HOOK_FILES"),
        "XPIA_HOOK_FILES constant must be removed from types.rs — \
         XPIA security is implemented by the Rust amplihack-hooks binary, \
         not by .py/.sh script files"
    );
}

#[test]
fn verify_framework_assets_does_not_check_individual_xpia_files() {
    // After the change, verify_framework_assets should NOT iterate over
    // individual hook files in the xpia directory. It should only check
    // whether the xpia directory exists.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let settings_src = fs::read_to_string(
        std::path::Path::new(manifest_dir).join("src/commands/install/settings.rs"),
    )
    .expect("settings.rs must be readable");

    // The per-file loop `for file in XPIA_HOOK_FILES` must be gone
    assert!(
        !settings_src.contains("XPIA_HOOK_FILES"),
        "verify_framework_assets must not reference XPIA_HOOK_FILES — \
         the per-file verification loop should be removed"
    );

    // Should still have the xpia_dir.exists() check (informational)
    assert!(
        settings_src.contains("xpia_dir") || settings_src.contains("xpia_hooks_dir"),
        "verify_framework_assets should still check for xpia directory existence"
    );
}

#[test]
fn xpia_hooks_dir_docstring_has_no_python_reference() {
    // After the change, the xpia_hooks_dir() docstring should not mention *.py files.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let paths_src = fs::read_to_string(
        std::path::Path::new(manifest_dir).join("src/commands/install/paths.rs"),
    )
    .expect("paths.rs must be readable");

    // Find the docstring for xpia_hooks_dir (between /// comments before the fn)
    let fn_idx = paths_src
        .find("fn xpia_hooks_dir")
        .expect("xpia_hooks_dir function must exist");
    let preceding = &paths_src[..fn_idx];

    assert!(
        !preceding.contains("*.py"),
        "xpia_hooks_dir docstring must not reference *.py files — \
         Python XPIA hooks are dead legacy code"
    );
}

#[test]
fn verify_framework_assets_succeeds_with_empty_xpia_dir() {
    // After the change, verify_framework_assets should succeed when the
    // xpia directory exists but contains NO script files — because the
    // Rust binary handles everything.
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    // Create staged assets
    create_minimal_staged_assets(temp.path());

    // Create the xpia hooks directory but leave it EMPTY
    let xpia_dir = temp.path().join(".amplihack/.claude/tools/xpia/hooks");
    fs::create_dir_all(&xpia_dir).unwrap();

    let result = settings::verify_framework_assets(&temp.path().join(".amplihack/.claude"));

    crate::test_support::restore_home(previous);

    // Must succeed — no individual file checks
    assert!(
        result.is_ok(),
        "verify_framework_assets must succeed with empty xpia dir — \
         it should not check for individual .py/.sh files: {:?}",
        result.err()
    );
}

#[test]
fn xpia_hook_specs_use_binary_subcmd_not_scripts() {
    // XPIA_HOOK_SPECS must remain intact and use BinarySubcmd exclusively.
    // This is the CORRECT way XPIA hooks are implemented — via the Rust binary.
    for spec in XPIA_HOOK_SPECS {
        match &spec.cmd {
            HookCommandKind::BinarySubcmd { subcmd } => {
                assert!(
                    !subcmd.is_empty(),
                    "XPIA hook spec for '{}' must have a non-empty subcmd",
                    spec.event
                );
                assert!(
                    !subcmd.contains(".py") && !subcmd.contains(".sh"),
                    "XPIA hook spec subcmd must not reference script files: {subcmd}"
                );
            }
        }
    }

    // Verify the three expected XPIA events are present
    let events: Vec<&str> = XPIA_HOOK_SPECS.iter().map(|s| s.event).collect();
    assert!(
        events.contains(&"SessionStart"),
        "XPIA must include SessionStart"
    );
    assert!(
        events.contains(&"PreToolUse"),
        "XPIA must include PreToolUse"
    );
    assert!(
        events.contains(&"PostToolUse"),
        "XPIA must include PostToolUse"
    );
}

#[test]
fn no_production_code_references_xpia_hook_files() {
    // Comprehensive check: no non-test production code should reference
    // XPIA_HOOK_FILES after the cleanup.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let install_dir = std::path::Path::new(manifest_dir).join("src/commands/install");

    for entry in fs::read_dir(&install_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "rs")
            && !path.to_str().unwrap_or("").contains("tests")
        {
            let content = fs::read_to_string(&path).unwrap();
            assert!(
                !content.contains("XPIA_HOOK_FILES"),
                "Production file {} must not reference XPIA_HOOK_FILES",
                path.display()
            );
        }
    }
}
