use super::*;
use crate::test_support::env_lock;
use amplihack_utils::launcher_context::{LauncherContext, write_launcher_context};
use std::collections::BTreeMap;
use std::fs;

#[test]
fn unknown_launcher_by_default() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _authoritative_host = crate::test_support::EnvVarGuard::unset("AMPLIHACK_AGENT_BINARY");
    set_launcher_env(None, None, None, None);
    let result = detect_launcher();
    assert!(
        matches!(result, LauncherType::Unknown),
        "Expected Unknown when no launcher env vars set, got: {result:?}"
    );
}

#[test]
fn detects_copilot_from_persisted_launcher_context() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _authoritative_host = crate::test_support::EnvVarGuard::unset("AMPLIHACK_AGENT_BINARY");
    set_launcher_env(None, None, None, None);
    let dir = tempfile::tempdir().unwrap();
    write_launcher_context(
        dir.path(),
        LauncherKind::Copilot,
        "amplihack copilot",
        BTreeMap::from([("AMPLIHACK_LAUNCHER".to_string(), "copilot".to_string())]),
    )
    .unwrap();

    let result = detect_launcher_for_dirs(&ProjectDirs::new(dir.path()));

    assert!(matches!(result, LauncherType::Copilot));
}

#[test]
fn detects_copilot_from_persisted_launcher_context_when_agent_cwd_is_nested() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _authoritative_host = crate::test_support::EnvVarGuard::unset("AMPLIHACK_AGENT_BINARY");
    set_launcher_env(None, None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("worktrees/feature/subdir");
    fs::create_dir_all(&nested).unwrap();
    write_launcher_context(
        dir.path(),
        LauncherKind::Copilot,
        "amplihack copilot",
        BTreeMap::from([("AMPLIHACK_LAUNCHER".to_string(), "copilot".to_string())]),
    )
    .unwrap();

    let result = detect_launcher_for_dirs(&ProjectDirs::new(&nested));

    assert!(
        matches!(result, LauncherType::Copilot),
        "PreToolUse launcher detection must resolve persisted context from an ancestor repo root when recipe agent subprocesses run from nested CWDs; got {result:?}"
    );
}

#[test]
fn ignores_stale_persisted_launcher_context() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _authoritative_host = crate::test_support::EnvVarGuard::unset("AMPLIHACK_AGENT_BINARY");
    set_launcher_env(None, None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    fs::create_dir_all(&dirs.runtime).unwrap();
    fs::write(
        dirs.launcher_context_file(),
        serde_json::to_string_pretty(&LauncherContext {
            launcher: LauncherKind::Copilot,
            command: "amplihack copilot".to_string(),
            timestamp: "2000-01-01T00:00:00+00:00".to_string(),
            environment: BTreeMap::new(),
        })
        .unwrap(),
    )
    .unwrap();

    let result = detect_launcher_for_dirs(&dirs);

    assert!(matches!(result, LauncherType::Unknown));
}

#[test]
fn codex_authoritative_host_overrides_inherited_provider_markers() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _codex = crate::test_support::EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "codex");
    let _copilot = crate::test_support::EnvVarGuard::set("GITHUB_COPILOT_AGENT", "1");
    let _claude = crate::test_support::EnvVarGuard::set("CLAUDE_CODE_SESSION", "inherited");
    assert_eq!(format!("{:?}", detect_launcher()), "Codex");
}

#[test]
fn codex_persisted_context_is_recognized_without_raw_input_side_effects() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _copilot = crate::test_support::EnvVarGuard::unset("GITHUB_COPILOT_AGENT");
    let _copilot_alt = crate::test_support::EnvVarGuard::unset("COPILOT_AGENT");
    let _amplifier = crate::test_support::EnvVarGuard::unset("AMPLIFIER_SESSION");
    let _claude = crate::test_support::EnvVarGuard::unset("CLAUDE_CODE_SESSION");
    let _claude_alt = crate::test_support::EnvVarGuard::unset("CLAUDE_SESSION_ID");
    // Empty authoritative marker should not mask the persisted context.
    let _host = crate::test_support::EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "");
    let dir = tempfile::tempdir().unwrap();
    write_launcher_context(
        dir.path(),
        LauncherKind::Codex,
        "amplihack codex",
        BTreeMap::new(),
    )
    .unwrap();
    let before = fs::read_dir(dir.path()).unwrap().count();
    let result = detect_launcher_for_dirs(&ProjectDirs::new(dir.path()));
    assert_eq!(format!("{result:?}"), "Codex");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before);
    assert!(!dir.path().join("AGENTS.md").exists());
}
