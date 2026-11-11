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

    let settings_path = temp.path().join(".claude/settings.json");
    let result =
        settings::ensure_settings_json(&settings_path, &staging_dir, 99999, &hooks_bin, None);

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

    let settings_path = temp.path().join(".claude/settings.json");
    let result =
        settings::ensure_settings_json(&settings_path, &staging_dir, 99999, &hooks_bin, None)
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

    let settings_path = temp.path().join(".claude/settings.json");
    let result =
        settings::ensure_settings_json(&settings_path, &staging_dir, 99999, &hooks_bin, None)
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

    let drift = settings::validate_amplihack_native_hook_contract(&settings_val, true);

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
        async_rewake: false,
    };
    wrappers.push(hooks::build_hook_wrapper(&drift_spec, &hooks_bin));

    let drift = settings::validate_amplihack_native_hook_contract(&settings_val, true);

    // Issue #2279: drift lines name events, never commands or content.
    assert!(
        drift
            .iter()
            .any(|issue| issue.contains("differ") && issue.contains("UserPromptSubmit")),
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
    let settings_path = temp.path().join(".claude/settings.json");
    let _ =
        settings::ensure_settings_json(&settings_path, &staging_dir, timestamp, &hooks_bin, None);

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

// ─── XPIA Python removal contracts ──────────────────────────────────────────

#[test]
fn xpia_hook_files_constant_must_not_exist() {
    // XPIA_HOOK_FILES must stay deleted from types.rs. Verify the constant is
    // not referenced anywhere in production code by checking the source file.
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
    // verify_framework_assets must not iterate over individual hook files in
    // the xpia directory. It should only check whether the xpia directory exists.
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
    // The xpia_hooks_dir() docstring should not mention *.py files.
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
    // verify_framework_assets should succeed when the xpia directory exists
    // but contains NO script files because the Rust binary handles everything.
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
fn no_production_code_references_xpia_hook_files() {
    // Comprehensive check: no non-test production code should reference
    // XPIA_HOOK_FILES after the cleanup.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let install_dir = std::path::Path::new(manifest_dir).join("src/commands/install");

    for entry in fs::read_dir(&install_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs")
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

#[test]
fn post_update_install_softens_known_transitional_xpia_asset_errors() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let settings_src = fs::read_to_string(
        std::path::Path::new(manifest_dir).join("src/commands/install/settings.rs"),
    )
    .expect("settings.rs must be readable");

    assert!(
        settings_src.contains("AMPLIHACK_POST_UPDATE_INSTALL"),
        "verify_framework_assets must inspect AMPLIHACK_POST_UPDATE_INSTALL so update-launched installs can distinguish transitional old-binary asset gaps from normal install failures"
    );
    assert!(
        settings_src.contains("self-heal on next invocation")
            || settings_src.contains("self-heal")
            || settings_src.contains("self heal"),
        "post-update missing known XPIA shell assets should be reported as self-healing on next invocation rather than misleading ❌ hard errors"
    );
}

// ─── Issue #1119: repo-local scope honors the caller-injected path ───────────

#[test]
fn ensure_settings_json_writes_to_repo_local_path_and_leaves_global_untouched() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let repo = temp.path().join("project");
    let repo_settings = repo.join(".claude").join("settings.json");
    let staging_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&staging_dir).unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let result =
        settings::ensure_settings_json(&repo_settings, &staging_dir, 99999, &hooks_bin, None)
            .expect("repo-local settings write should succeed");

    crate::test_support::restore_home(previous);

    assert!(result.0, "repo-local settings setup must succeed");
    assert!(
        repo_settings.exists(),
        "settings.json must be written under the repo-local .claude directory"
    );
    assert!(
        !temp.path().join(".claude/settings.json").exists(),
        "repo-local install must not create the global ~/.claude/settings.json"
    );
}

#[cfg(unix)]
#[test]
fn ensure_settings_json_repo_local_backup_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let repo_claude = temp.path().join("project").join(".claude");
    fs::create_dir_all(&repo_claude).unwrap();
    let repo_settings = repo_claude.join("settings.json");
    // Pre-existing settings force a backup to be created.
    fs::write(&repo_settings, "{}").unwrap();

    let staging_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&staging_dir).unwrap();
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let timestamp = 1_700_000_042_u64;
    let _ =
        settings::ensure_settings_json(&repo_settings, &staging_dir, timestamp, &hooks_bin, None)
            .expect("repo-local settings write should succeed");

    crate::test_support::restore_home(previous);

    let backup_path = repo_claude.join(format!("settings.json.backup.{timestamp}"));
    assert!(
        backup_path.exists(),
        "a backup must be created when repo-local settings.json already exists"
    );
    let mode = fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "repo-local backup must remain owner-read/write only (0o600), got {mode:o}"
    );
}

// ─── Issue #1745: the `statusLine` entry in ~/.claude/settings.json ──────────

mod status_line {
    use super::*;
    use crate::commands::install::settings::{StatusLineOutcome, ensure_status_line};
    use serde_json::{Value, json};
    use std::path::Path;

    const CMD: &str = "'/usr/local/bin/amplihack' profile statusline";

    fn entry(cmd: &str) -> Value {
        json!({"type": "command", "command": cmd})
    }

    #[test]
    fn no_entry_is_installed() {
        let mut v = json!({"theme": "dark"});
        assert_eq!(
            ensure_status_line(&mut v, CMD),
            StatusLineOutcome::Installed
        );
        assert_eq!(v, json!({"theme": "dark", "statusLine": entry(CMD)}));
    }

    #[test]
    fn an_older_amplihack_entry_is_updated_keeping_padding() {
        for old in [
            "'/old/bin/amplihack' profile statusline",
            "  '/old/bin/amplihack' profile statusline  ",
            "'/o'\\''x/amplihack' profile statusline",
            "amplihack profile statusline",
            "~/.amplihack/.claude/tools/statusline.sh",
            "$HOME/.amplihack/.claude/tools/statusline.sh",
            "\"${HOME}/.amplihack/.claude/tools/statusline.sh\"",
            "/home/u/.amplihack/.claude/tools/statusline.sh",
            "'/home/u/.amplihack/.claude/tools/statusline.sh'",
        ] {
            let mut v = json!({
                "statusLine": {"type": "command", "command": old, "padding": 2},
                "theme": "dark",
            });
            assert_eq!(
                ensure_status_line(&mut v, CMD),
                StatusLineOutcome::Updated,
                "{old}"
            );
            assert_eq!(
                v,
                json!({
                    "statusLine": {"type": "command", "command": CMD, "padding": 2},
                    "theme": "dark",
                }),
                "{old}"
            );
        }
    }

    #[test]
    fn an_update_keeps_padding_and_every_other_key() {
        let mut v = json!({"statusLine": {
            "type": "command",
            "command": "'/old/bin/amplihack' profile statusline",
            "padding": 1,
            "refreshInterval": 5,
        }});
        assert_eq!(ensure_status_line(&mut v, CMD), StatusLineOutcome::Updated);
        assert_eq!(
            v,
            json!({"statusLine": {
                "type": "command",
                "command": CMD,
                "padding": 1,
                "refreshInterval": 5,
            }})
        );
    }

    #[test]
    fn only_global_scope_asks_for_the_binary() {
        use crate::commands::install::interactive::HookScope;
        use crate::commands::install::status_line_command;
        for (scope, want) in [
            (HookScope::Global, Some(CMD.to_string())),
            (HookScope::RepoLocal, None),
        ] {
            let mut asked = false;
            let got = status_line_command(scope, || {
                asked = true;
                Ok("/usr/local/bin/amplihack".into())
            });
            assert_eq!(got, want, "{scope:?}");
            assert_eq!(asked, want.is_some(), "{scope:?}");
        }
    }

    #[test]
    fn a_failed_binary_lookup_writes_no_entry() {
        use crate::commands::install::interactive::HookScope;
        use crate::commands::install::status_line_command;
        let got = status_line_command(HookScope::Global, || {
            anyhow::bail!("binary is writable by others")
        });
        assert_eq!(got, None);
    }

    #[test]
    fn a_binary_path_with_a_newline_writes_no_entry() {
        // SR6: no entry, and no fallback to a bare `amplihack` on PATH.
        use crate::commands::install::interactive::HookScope;
        use crate::commands::install::status_line_command;
        for path in ["/opt/a\nb/amplihack", "/opt/a\rb/amplihack"] {
            let got = status_line_command(HookScope::Global, || Ok(path.into()));
            assert_eq!(got, None, "{path:?}");
        }
    }

    #[test]
    fn the_current_entry_is_unchanged() {
        let mut v = json!({"statusLine": entry(CMD)});
        let before = v.clone();
        assert_eq!(
            ensure_status_line(&mut v, CMD),
            StatusLineOutcome::Unchanged
        );
        assert_eq!(v, before);
    }

    #[test]
    fn a_user_entry_is_kept() {
        for user in [
            entry("$CLAUDE_PROJECT_DIR/.claude/tools/statusline.sh"),
            entry("printf 'mine'"),
            entry("'/x/amplihack' profile statusline --root '/r' --owner 'work'"),
            entry("/old/bin/amplihack profile statusline --root '/r'"),
            entry("'/x/not-amplihack' profile statusline"),
            entry("'relative/amplihack' profile statusline"),
            entry("bash ~/.amplihack/.claude/tools/statusline.sh"),
            entry("~/.amplihack/.claude/tools/statusline.sh; rm -rf ~"),
            entry("/a b/.amplihack/.claude/tools/statusline.sh"),
            json!("a string"),
            json!(null),
            json!(42),
            json!({"type": "command"}),
            json!({"type": "command", "command": 5}),
            json!([CMD]),
        ] {
            let mut v = json!({"statusLine": user.clone(), "theme": "dark"});
            let before = v.clone();
            assert_eq!(
                ensure_status_line(&mut v, CMD),
                StatusLineOutcome::KeptUser,
                "{user}"
            );
            assert_eq!(v, before, "{user}");
        }
    }

    #[test]
    fn a_pipeline_around_the_legacy_script_is_the_users() {
        let mine = entry("bash ~/.amplihack/.claude/tools/statusline.sh | my-filter");
        let mut v = json!({"statusLine": mine.clone()});
        assert_eq!(ensure_status_line(&mut v, CMD), StatusLineOutcome::KeptUser);
        assert_eq!(v["statusLine"], mine);
    }

    #[test]
    fn a_command_that_only_contains_profile_statusline_is_the_users() {
        let mine = entry("my-profile statusline-helper.sh");
        let mut v = json!({"statusLine": mine.clone()});
        assert_eq!(ensure_status_line(&mut v, CMD), StatusLineOutcome::KeptUser);
        assert_eq!(v["statusLine"], mine);
    }

    #[test]
    fn running_twice_is_idempotent() {
        let mut v = json!({});
        ensure_status_line(&mut v, CMD);
        let once = v.clone();
        assert_eq!(
            ensure_status_line(&mut v, CMD),
            StatusLineOutcome::Unchanged
        );
        assert_eq!(v, once);
    }

    #[test]
    fn outcomes_print_at_most_one_line_each() {
        assert_eq!(
            StatusLineOutcome::Installed.message(),
            Some("statusLine: installed")
        );
        assert_eq!(
            StatusLineOutcome::Updated.message(),
            Some("statusLine: updated")
        );
        assert_eq!(StatusLineOutcome::Unchanged.message(), None);
        assert_eq!(
            StatusLineOutcome::KeptUser.message(),
            Some("statusLine: kept your existing entry")
        );
    }

    // ── Through ensure_settings_json ────────────────────────────────────────

    /// Runs `ensure_settings_json` on `settings_path` under a temp `HOME`
    /// and returns the parsed result.
    fn run(temp: &Path, settings_path: &Path, status_line: Option<&str>) -> Value {
        let staging_dir = temp.join(".amplihack/.claude");
        fs::create_dir_all(&staging_dir).unwrap();
        let hooks_bin = create_exe_stub(temp, "amplihack-hooks");
        settings::ensure_settings_json(settings_path, &staging_dir, 99999, &hooks_bin, status_line)
            .expect("settings write should succeed");
        serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap()
    }

    fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        let out = f(temp.path());
        crate::test_support::restore_home(previous);
        out
    }

    #[test]
    fn explicit_install_writes_one_entry_and_is_idempotent() {
        with_home(|home| {
            let path = home.join(".claude/settings.json");
            let first = run(home, &path, Some(CMD));
            assert_eq!(first["statusLine"], entry(CMD));
            let second = run(home, &path, Some(CMD));
            assert_eq!(first, second);
            let raw = fs::read_to_string(&path).unwrap();
            assert_eq!(raw.matches("\"statusLine\"").count(), 1, "{raw}");
        });
    }

    #[test]
    fn a_user_entry_and_other_keys_survive_install() {
        with_home(|home| {
            let path = home.join(".claude/settings.json");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mine = json!({"type": "command", "command": "printf 'mine'", "padding": 0});
            fs::write(
                &path,
                json!({"statusLine": mine, "model": "opus", "env": {"A": "1"}}).to_string(),
            )
            .unwrap();
            let after = run(home, &path, Some(CMD));
            assert_eq!(after["statusLine"], mine);
            assert_eq!(after["model"], "opus");
            assert_eq!(after["env"], json!({"A": "1"}));
        });
    }

    // A repo-local install is the only production caller that passes `None`.
    // The launch repair passes `Some(cmd)`; it is tested through
    // `ensure_framework_installed_with` in `issue_1745_launch_repair_statusline`.

    #[test]
    fn a_repo_local_install_keeps_an_amplihack_entry() {
        with_home(|home| {
            let path = home.join(".claude/settings.json");
            run(home, &path, Some(CMD));
            let after = run(home, &path, None);
            assert_eq!(after["statusLine"], entry(CMD), "None never removes it");
        });
    }

    #[test]
    fn a_repo_local_install_never_adds_an_entry() {
        with_home(|home| {
            let path = home.join(".claude/settings.json");
            let after = run(home, &path, None);
            assert!(after.get("statusLine").is_none(), "{after}");
        });
    }

    #[test]
    fn an_update_through_the_file_keeps_padding_and_other_keys() {
        with_home(|home| {
            let path = home.join(".claude/settings.json");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let old = json!({
                "type": "command",
                "command": "'/old/bin/amplihack' profile statusline",
                "padding": 3,
                "refreshInterval": 7,
            });
            fs::write(
                &path,
                json!({"statusLine": old, "model": "opus"}).to_string(),
            )
            .unwrap();
            let after = run(home, &path, Some(CMD));
            assert_eq!(
                after["statusLine"],
                json!({"type": "command", "command": CMD, "padding": 3, "refreshInterval": 7})
            );
            assert_eq!(after["model"], "opus");
        });
    }

    #[test]
    fn a_repo_local_target_gains_no_entry() {
        with_home(|home| {
            let path = home.join("project/.claude/settings.json");
            let after = run(home, &path, None);
            assert!(after.get("statusLine").is_none(), "{after}");
            assert!(!home.join(".claude/settings.json").exists());
        });
    }
}
