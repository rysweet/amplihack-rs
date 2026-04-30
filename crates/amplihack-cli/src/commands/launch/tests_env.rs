use super::*;
use crate::binary_finder::BinaryInfo;
use crate::env_builder::EnvBuilder;
use crate::launcher_context::{LauncherKind, read_launcher_context};
use crate::test_support::{home_env_lock, restore_cwd, restore_home, set_cwd, set_home};
use std::fs;
use std::path::PathBuf;

#[test]
fn build_command_injects_uvx_plugin_and_project_args_for_claude() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let execution_dir = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    let previous_original_cwd = std::env::var_os("AMPLIHACK_ORIGINAL_CWD");
    unsafe {
        std::env::set_var("UV_PYTHON", "1");
        std::env::remove_var("AMPLIHACK_ORIGINAL_CWD");
    }

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    let cmd = build_command_for_dir(
        &binary,
        false,
        false,
        false,
        &[],
        Some(execution_dir.path()),
    );
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }
    match previous_original_cwd {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ORIGINAL_CWD", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ORIGINAL_CWD") },
    }

    assert_eq!(args[0], "--plugin-dir");
    assert_eq!(
        args[1],
        home.path()
            .join(".amplihack")
            .join(".claude")
            .display()
            .to_string()
    );
    assert_eq!(args[2], "--add-dir");
    assert_eq!(args[3], execution_dir.path().display().to_string());
    assert_eq!(args[4], "--model");
}

#[test]
fn build_command_prefers_original_cwd_for_staged_uvx_launches() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let execution_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    let previous_original_cwd = std::env::var_os("AMPLIHACK_ORIGINAL_CWD");
    let previous_is_staged = std::env::var_os("AMPLIHACK_IS_STAGED");
    unsafe {
        std::env::set_var("UV_PYTHON", "1");
        std::env::set_var("AMPLIHACK_ORIGINAL_CWD", project_dir.path());
        std::env::set_var("AMPLIHACK_IS_STAGED", "1");
    }

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    let cmd = build_command_for_dir(
        &binary,
        false,
        false,
        false,
        &[],
        Some(execution_dir.path()),
    );
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }
    match previous_original_cwd {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ORIGINAL_CWD", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ORIGINAL_CWD") },
    }
    match previous_is_staged {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_IS_STAGED", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_IS_STAGED") },
    }

    assert_eq!(args[0], "--plugin-dir");
    assert_eq!(args[2], "--add-dir");
    assert_eq!(args[3], project_dir.path().display().to_string());
}

#[test]
fn build_command_does_not_duplicate_uvx_plugin_or_add_dir_args() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    unsafe { std::env::set_var("UV_PYTHON", "1") };

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    let extra = vec![
        "--plugin-dir".to_string(),
        "/custom/plugin".to_string(),
        "--add-dir".to_string(),
        "/custom/project".to_string(),
    ];
    let cmd = build_command(&binary, false, false, false, &extra);
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }

    assert_eq!(
        args,
        vec![
            "--model",
            "opus[1m]",
            "--plugin-dir",
            "/custom/plugin",
            "--add-dir",
            "/custom/project",
        ]
    );
}

#[test]
fn augment_claude_launch_env_sets_directory_copy_plugin_root_and_npm_bin() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());
    let previous_plugin_installed = std::env::var_os("AMPLIHACK_PLUGIN_INSTALLED");
    unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") };

    let env = augment_claude_launch_env(EnvBuilder::new(), "claude").build();

    restore_home(original_home);
    match previous_plugin_installed {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") },
    }

    let expected_plugin_root = home.path().join(".amplihack").join(".claude");
    let expected_plugin_root = expected_plugin_root.display().to_string();
    assert_eq!(
        env.get("CLAUDE_PLUGIN_ROOT").map(String::as_str),
        Some(expected_plugin_root.as_str())
    );
    let path = env.get("PATH").expect("PATH should be populated");
    assert!(
        path.split(':')
            .next()
            .unwrap_or_default()
            .ends_with(".npm-global/bin"),
        "expected ~/.npm-global/bin to be prepended to PATH, got {path}"
    );
}

#[test]
fn augment_claude_launch_env_prefers_installed_plugin_cache_path() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let previous_plugin_installed = std::env::var_os("AMPLIHACK_PLUGIN_INSTALLED");
    unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", "true") };

    let env = augment_claude_launch_env(EnvBuilder::new(), "claude").build();

    restore_home(original_home);
    match previous_plugin_installed {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") },
    }

    let expected_plugin_root = home
        .path()
        .join(".claude")
        .join("plugins")
        .join("cache")
        .join("amplihack")
        .join("amplihack")
        .join("0.9.0");
    let expected_plugin_root = expected_plugin_root.display().to_string();
    assert_eq!(
        env.get("CLAUDE_PLUGIN_ROOT").map(String::as_str),
        Some(expected_plugin_root.as_str())
    );
}

#[test]
fn persist_launcher_context_writes_copilot_context_file() {
    let dir = tempfile::tempdir().unwrap();
    let args = vec!["--model".to_string(), "opus".to_string()];

    persist_launcher_context("copilot", Some(dir.path()), &args).unwrap();

    let context = read_launcher_context(dir.path()).unwrap();
    assert_eq!(context.launcher, LauncherKind::Copilot);
    assert_eq!(context.command, "amplihack copilot --model opus");
    assert_eq!(
        context
            .environment
            .get("AMPLIHACK_LAUNCHER")
            .map(String::as_str),
        Some("copilot")
    );
}

/// Issue #506 regression: when launching with `--tool copilot`, the
/// persisted `launcher_context.json` must include
/// `AMPLIHACK_AGENT_BINARY=copilot` in its environment map so nested
/// re-launches (recipe runner sub-recipes, agent tasks) inherit the
/// correct active binary instead of silently falling back to claude.
///
/// Per Decision 2 in the requirements doc the value is hardcoded —
/// `persist_launcher_context` only runs when `tool == "copilot"`, so
/// reading from `std::env::var` would be wrong (the env var may be
/// missing on the parent process even though we know we are launching
/// copilot). The test is intentionally tight: it asserts the exact key
/// and value, so any future "lost in translation" change at this
/// chokepoint surfaces immediately.
///
/// TDD note: this test is expected to FAIL until the implementation
/// adds the explicit `environment.insert("AMPLIHACK_AGENT_BINARY", …)`
/// line in `persist_launcher_context`.
#[test]
fn persist_launcher_context_writes_agent_binary_for_copilot() {
    let dir = tempfile::tempdir().unwrap();

    persist_launcher_context("copilot", Some(dir.path()), &[]).unwrap();

    let context = read_launcher_context(dir.path()).unwrap();
    assert_eq!(context.launcher, LauncherKind::Copilot);
    assert_eq!(
        context
            .environment
            .get("AMPLIHACK_AGENT_BINARY")
            .map(String::as_str),
        Some("copilot"),
        "issue #506: persisted launcher context must propagate \
         AMPLIHACK_AGENT_BINARY=copilot so nested launches stay on the \
         copilot binary; got environment={:?}",
        context.environment
    );
    // Defense-in-depth: AMPLIHACK_LAUNCHER must remain alongside the new
    // AMPLIHACK_AGENT_BINARY entry — they are independent contracts and
    // the fix must add, not replace.
    assert_eq!(
        context
            .environment
            .get("AMPLIHACK_LAUNCHER")
            .map(String::as_str),
        Some("copilot"),
        "AMPLIHACK_LAUNCHER must still be present after #506 fix"
    );
}

/// Issue #506 scope guard: the fix must NOT start persisting a launcher
/// context for non-copilot tools. The early-return at context.rs:14-16
/// is load-bearing — sub-launches under claude/codex/amplifier rely on
/// the absence of a stale context file. Asserting "no file written"
/// keeps the scope of #506 tight.
#[test]
fn persist_launcher_context_writes_nothing_for_non_copilot_tools() {
    let dir = tempfile::tempdir().unwrap();

    persist_launcher_context("claude", Some(dir.path()), &[]).unwrap();
    persist_launcher_context("codex", Some(dir.path()), &[]).unwrap();
    persist_launcher_context("amplifier", Some(dir.path()), &[]).unwrap();

    assert!(
        read_launcher_context(dir.path()).is_none(),
        "issue #506 fix must remain copilot-only — no context file may \
         be written for claude/codex/amplifier launches"
    );
}

#[test]
fn build_command_skips_dangerous_flag_for_copilot() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    unsafe { std::env::set_var("UV_PYTHON", "1") };

    let binary = BinaryInfo {
        name: "copilot".to_string(),
        path: PathBuf::from("/usr/bin/copilot"),
        version: None,
    };
    // skip_permissions = true, but tool is copilot → no --dangerously-skip-permissions
    let cmd = build_command(&binary, false, false, true, &[]);
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }

    assert!(
        !args.contains(&"--dangerously-skip-permissions".to_string()),
        "copilot must not receive --dangerously-skip-permissions, got: {args:?}"
    );
    // Copilot should also not get the Claude default model
    assert!(
        !args.iter().any(|a| a == "opus[1m]"),
        "copilot must not get Claude's default model, got: {args:?}"
    );
}

#[test]
fn build_command_injects_dangerous_flag_for_claude() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    unsafe { std::env::set_var("UV_PYTHON", "1") };

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    // skip_permissions = true + tool is claude → should inject
    let cmd = build_command(&binary, false, false, true, &[]);
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }

    assert!(
        args.contains(&"--dangerously-skip-permissions".to_string()),
        "claude should receive --dangerously-skip-permissions, got: {args:?}"
    );
}
