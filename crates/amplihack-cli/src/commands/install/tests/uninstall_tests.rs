use super::helpers::*;
use super::*;
use std::fs;

// ─── TDD: Group 15 — run_uninstall removes binaries (Phase 3) ────────────

#[test]
fn run_uninstall_removes_binaries_listed_in_manifest() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    fs::create_dir_all(&local_bin).unwrap();
    let hooks_binary = local_bin.join("amplihack-hooks");
    fs::write(&hooks_binary, "#!/bin/bash\n").unwrap();
    assert!(hooks_binary.exists());

    fs::create_dir_all(temp.path().join(".amplihack/.claude/install")).unwrap();
    let manifest_json = serde_json::json!({
        "files": [],
        "dirs": [],
        "binaries": [hooks_binary.to_string_lossy()],
        "hook_registrations": []
    });
    fs::write(
        temp.path()
            .join(".amplihack/.claude/install/amplihack-manifest.json"),
        serde_json::to_string_pretty(&manifest_json).unwrap(),
    )
    .unwrap();

    run_uninstall().unwrap();

    crate::test_support::restore_home(previous);

    assert!(
        !hooks_binary.exists(),
        "amplihack-hooks must be removed by uninstall Phase 3"
    );
}

// ─── TDD: Group 16 — remove_hook_registrations ───────────────────────────

#[test]
fn remove_hook_registrations_removes_amplihack_hooks_entries() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");

    let settings_val = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.local/bin/amplihack-hooks session-start",
                        "timeout": 10
                    }]
                },
                {
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.local/bin/some-other-tool start",
                        "timeout": 10
                    }]
                }
            ]
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string(&settings_val).unwrap(),
    )
    .unwrap();

    remove_hook_registrations(&settings_path).unwrap();

    let updated_raw = fs::read_to_string(&settings_path).unwrap();
    let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

    let session_hooks = updated["hooks"]["SessionStart"].as_array().unwrap();

    for wrapper in session_hooks {
        if let Some(hooks_arr) = wrapper.get("hooks").and_then(serde_json::Value::as_array) {
            for hook in hooks_arr {
                let cmd = hook["command"].as_str().unwrap_or("");
                assert!(
                    !cmd.contains("amplihack-hooks"),
                    "amplihack-hooks command must be removed, found: {cmd}"
                );
            }
        }
    }

    assert_eq!(
        session_hooks.len(),
        1,
        "non-amplihack hook must remain; only amplihack-hooks entry removed"
    );
}

#[test]
fn remove_hook_registrations_removes_tools_amplihack_python_paths() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");

    let settings_val = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": [
                {
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py",
                        "timeout": 5
                    }]
                }
            ]
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string(&settings_val).unwrap(),
    )
    .unwrap();

    remove_hook_registrations(&settings_path).unwrap();

    let updated_raw = fs::read_to_string(&settings_path).unwrap();
    let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

    let any_amplihack_path = match updated["hooks"]["UserPromptSubmit"].as_array() {
        None => false,
        Some(hooks_arr) => hooks_arr.iter().any(|wrapper| {
            wrapper
                .get("hooks")
                .and_then(serde_json::Value::as_array)
                .map(|hooks_inner| {
                    hooks_inner.iter().any(|h| {
                        h["command"]
                            .as_str()
                            .map(|c| c.contains("tools/amplihack/"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        }),
    };
    assert!(
        !any_amplihack_path,
        "tools/amplihack/ Python hook paths must be removed from settings.json"
    );
}

#[test]
fn remove_hook_registrations_preserves_non_amplihack_entries() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");

    let settings_val = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/pre_tool_use.py"
                    }]
                },
                {
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.amplihack/.claude/tools/xpia/hooks/pre_tool_use.py"
                    }]
                }
            ]
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string(&settings_val).unwrap(),
    )
    .unwrap();

    remove_hook_registrations(&settings_path).unwrap();

    let updated_raw = fs::read_to_string(&settings_path).unwrap();
    let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

    let hooks_arr = updated["hooks"]["PreToolUse"].as_array().unwrap();

    let xpia_present = hooks_arr.iter().any(|wrapper| {
        wrapper
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .map(|hooks_inner| {
                hooks_inner.iter().any(|h| {
                    h["command"]
                        .as_str()
                        .map(|c| c.contains("tools/xpia/"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });
    assert!(
        xpia_present,
        "XPIA hook entries must NOT be removed by remove_hook_registrations"
    );
}

// ─── TDD: Group 16b — remove_hook_registrations prunes empty arrays ─────────

#[test]
fn remove_hook_registrations_leaves_no_empty_arrays() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");

    let settings_val = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.local/bin/amplihack-hooks pre-tool-use"
                    }]
                }
            ],
            "SessionStart": [
                {
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.local/bin/amplihack-hooks session-start",
                        "timeout": 10
                    }]
                }
            ]
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string(&settings_val).unwrap(),
    )
    .unwrap();

    remove_hook_registrations(&settings_path).unwrap();

    let updated_raw = fs::read_to_string(&settings_path).unwrap();
    let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

    if let Some(hooks_map) = updated["hooks"].as_object() {
        for (event, wrappers_val) in hooks_map {
            if let Some(arr) = wrappers_val.as_array() {
                assert!(
                    !arr.is_empty(),
                    "Event type '{}' must be removed from hooks map when all its \
                     wrappers are gone, but found empty array. Full hooks: {}",
                    event,
                    serde_json::to_string_pretty(&updated["hooks"]).unwrap()
                );
            }
        }
    }
}

#[test]
fn remove_hook_registrations_mixed_event_keeps_non_amplihack_wrapper() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");

    let settings_val = serde_json::json!({
        "hooks": {
            "PostToolUse": [
                {
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.local/bin/amplihack-hooks post-tool-use"
                    }]
                },
                {
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.local/bin/third-party-tool post"
                    }]
                }
            ]
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string(&settings_val).unwrap(),
    )
    .unwrap();

    remove_hook_registrations(&settings_path).unwrap();

    let updated_raw = fs::read_to_string(&settings_path).unwrap();
    let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

    let wrappers = updated["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(
        wrappers.len(),
        1,
        "PostToolUse must retain the non-amplihack wrapper"
    );

    let cmd = wrappers[0]["hooks"][0]["command"].as_str().unwrap_or("");
    assert!(
        cmd.contains("third-party-tool"),
        "Remaining wrapper must be the third-party hook, got: {cmd}"
    );
}

// ─── TDD: Group 19 — run_uninstall dedup correctness ─────────────────────

#[test]
fn run_uninstall_handles_duplicate_dirs_in_manifest() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging = temp.path().join(".amplihack/.claude");
    let tracked_dir = staging.join("agents/amplihack");
    fs::create_dir_all(&tracked_dir).unwrap();
    fs::write(tracked_dir.join("dummy.txt"), "x").unwrap();

    fs::create_dir_all(staging.join("install")).unwrap();
    let manifest_val = InstallManifest {
        files: vec![],
        dirs: vec![
            "agents/amplihack".to_string(),
            "agents/amplihack".to_string(),
            "agents/amplihack".to_string(),
        ],
        binaries: vec![],
        hook_registrations: vec![],
    };
    manifest::write_manifest(
        &staging.join("install/amplihack-manifest.json"),
        &manifest_val,
    )
    .unwrap();

    let result = run_uninstall();

    crate::test_support::restore_home(previous);

    assert!(
        result.is_ok(),
        "run_uninstall must succeed with duplicate dir entries in manifest, got: {result:?}"
    );
    assert!(
        !tracked_dir.exists(),
        "tracked directory must be removed during uninstall"
    );
}
