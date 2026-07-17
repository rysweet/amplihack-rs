use super::*;
use crate::test_support::env_lock;
use amplihack_cli::launcher_context::{LauncherContext, write_launcher_context};
use std::collections::BTreeMap;
use std::fs;

#[test]
fn unknown_launcher_by_default() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
