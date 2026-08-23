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
        ..InstallManifest::default()
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

#[test]
fn claude_plugin_install_uninstall_reinstall_lifecycle() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let staged = temp.path().join(".amplihack/.claude");
    let create_staging = || {
        fs::create_dir_all(staged.join("skills/lifecycle")).unwrap();
        fs::create_dir_all(staged.join("install")).unwrap();
        fs::write(
            staged.join("skills/lifecycle/SKILL.md"),
            "---\nname: lifecycle\n---\n",
        )
        .unwrap();
        manifest::write_manifest(
            &staged.join("install/amplihack-manifest.json"),
            &InstallManifest::default(),
        )
        .unwrap();
    };
    create_staging();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    let direct_skill = temp.path().join(".claude/skills/lifecycle");
    let wrapper = temp.path().join(".claude/skills/amplihack");
    assert!(direct_skill.is_dir());
    assert!(wrapper.is_dir());

    run_uninstall().unwrap();
    assert!(!direct_skill.exists());
    assert!(!wrapper.exists());

    create_staging();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    crate::test_support::restore_home(previous);

    assert!(direct_skill.join("SKILL.md").is_file());
    assert!(wrapper.join(".claude-plugin/plugin.json").is_file());
}

#[test]
fn uninstall_preserves_replaced_managed_claude_skill() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let staged = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(staged.join("skills/replaced")).unwrap();
    fs::create_dir_all(staged.join("install")).unwrap();
    fs::write(staged.join("skills/replaced/SKILL.md"), "canonical").unwrap();
    manifest::write_manifest(
        &staged.join("install/amplihack-manifest.json"),
        &InstallManifest::default(),
    )
    .unwrap();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    let installed = temp.path().join(".claude/skills/replaced/SKILL.md");
    fs::write(&installed, "user replacement").unwrap();

    run_uninstall().unwrap();
    crate::test_support::restore_home(previous);

    assert_eq!(fs::read_to_string(installed).unwrap(), "user replacement");
}

#[test]
fn uninstall_preserves_replaced_claude_plugin_wrapper() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let staged = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(staged.join("skills/managed")).unwrap();
    fs::create_dir_all(staged.join("install")).unwrap();
    fs::write(staged.join("skills/managed/SKILL.md"), "canonical").unwrap();
    manifest::write_manifest(
        &staged.join("install/amplihack-manifest.json"),
        &InstallManifest::default(),
    )
    .unwrap();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    let replacement = temp.path().join(".claude/skills/amplihack/user.txt");
    fs::write(&replacement, "user replacement").unwrap();

    run_uninstall().unwrap();
    crate::test_support::restore_home(previous);

    assert_eq!(fs::read_to_string(replacement).unwrap(), "user replacement");
}

#[test]
fn uninstall_preserves_managed_skill_replaced_by_root_file_and_removes_rest() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let staged = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(staged.join("skills/replaced")).unwrap();
    fs::create_dir_all(staged.join("skills/removed")).unwrap();
    fs::create_dir_all(staged.join("install")).unwrap();
    fs::create_dir_all(temp.path().join(".amplihack/amplifier-bundle")).unwrap();
    fs::write(staged.join("skills/replaced/SKILL.md"), "canonical").unwrap();
    fs::write(staged.join("skills/removed/SKILL.md"), "canonical").unwrap();
    manifest::write_manifest(
        &staged.join("install/amplihack-manifest.json"),
        &InstallManifest::default(),
    )
    .unwrap();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    let replacement = temp.path().join(".claude/skills/replaced");
    fs::remove_dir_all(&replacement).unwrap();
    fs::write(&replacement, "user root file").unwrap();

    run_uninstall().unwrap();
    crate::test_support::restore_home(previous);

    assert_eq!(fs::read_to_string(&replacement).unwrap(), "user root file");
    assert!(!temp.path().join(".claude/skills/removed").exists());
    assert!(!temp.path().join(".claude/skills/amplihack").exists());
    assert!(!temp.path().join(".amplihack/amplifier-bundle").exists());
}

#[cfg(unix)]
#[test]
fn uninstall_preserves_plugin_wrapper_replaced_by_root_symlink_and_removes_rest() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let staged = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(staged.join("skills/removed")).unwrap();
    fs::create_dir_all(staged.join("install")).unwrap();
    fs::create_dir_all(temp.path().join(".amplihack/amplifier-bundle")).unwrap();
    fs::write(staged.join("skills/removed/SKILL.md"), "canonical").unwrap();
    manifest::write_manifest(
        &staged.join("install/amplihack-manifest.json"),
        &InstallManifest::default(),
    )
    .unwrap();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    let wrapper = temp.path().join(".claude/skills/amplihack");
    fs::remove_dir_all(&wrapper).unwrap();
    let user_target = temp.path().join("user-plugin-wrapper");
    fs::create_dir_all(&user_target).unwrap();
    fs::write(user_target.join("user.txt"), "preserve").unwrap();
    std::os::unix::fs::symlink(&user_target, &wrapper).unwrap();

    run_uninstall().unwrap();
    crate::test_support::restore_home(previous);

    assert!(wrapper.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(
        fs::read_to_string(user_target.join("user.txt")).unwrap(),
        "preserve"
    );
    assert!(!temp.path().join(".claude/skills/removed").exists());
    assert!(!temp.path().join(".amplihack/amplifier-bundle").exists());
}
