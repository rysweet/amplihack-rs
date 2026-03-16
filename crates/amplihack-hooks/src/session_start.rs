//! Session start hook: initializes session state and injects context.
//!
//! On session start, this hook:
//! 1. Checks for version mismatches
//! 2. Migrates global hooks if needed
//! 3. Captures original request
//! 4. Injects project context, learnings, and preferences
//! 5. Returns additional context for the session

use crate::protocol::{FailurePolicy, Hook};
use amplihack_cli::binary_finder::BinaryFinder;
use amplihack_cli::memory::{
    background_index_job_active, check_index_status, resolve_code_graph_db_path_for_project,
    summarize_code_graph,
};
use amplihack_state::AtomicJsonFile;
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct SessionStartHook;

impl Hook for SessionStartHook {
    fn name(&self) -> &'static str {
        "session_start"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        match &input {
            HookInput::SessionStart { .. } => {}
            _ => return Ok(Value::Object(serde_json::Map::new())),
        }

        let dirs = ProjectDirs::from_cwd();
        let mut context_parts: Vec<String> = Vec::new();

        // Load project context (PROJECT.md).
        if let Some(ctx) = load_project_context(&dirs) {
            context_parts.push(ctx);
        }

        // Load recent learnings/discoveries.
        if let Some(learnings) = load_discoveries(&dirs) {
            context_parts.push(learnings);
        }

        // Load user preferences.
        if let Some(prefs) = load_user_preferences(&dirs) {
            context_parts.push(prefs);
        }

        // Check for version mismatch natively.
        if let Some(version_notice) = check_version(&dirs) {
            context_parts.push(version_notice);
        }

        // Migrate global hooks if needed.
        if let Some(migration_notice) = migrate_global_hooks() {
            context_parts.push(migration_notice);
        }

        let blarify_indexing_active = match setup_blarify_indexing(&dirs) {
            Ok(active) => active,
            Err(err) => {
                tracing::warn!("Blarify setup failed (non-critical): {}", err);
                false
            }
        };

        if !blarify_indexing_active && let Some(code_graph_context) = load_code_graph_context(&dirs)
        {
            context_parts.push(code_graph_context);
        }

        let additional_context = context_parts.join("\n\n");

        if additional_context.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        Ok(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": additional_context
            }
        }))
    }
}

fn load_project_context(dirs: &ProjectDirs) -> Option<String> {
    let candidates = [dirs.root.join("PROJECT.md"), dirs.project_context()];

    for path in &candidates {
        if let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            return Some(format!("## Project Context\n\n{}", content.trim()));
        }
    }

    None
}

fn load_discoveries(dirs: &ProjectDirs) -> Option<String> {
    let path = dirs.root.join("DISCOVERIES.md");
    if let Ok(content) = fs::read_to_string(path)
        && !content.trim().is_empty()
    {
        return Some(format!("## Recent Learnings\n\n{}", content.trim()));
    }
    None
}

fn load_user_preferences(dirs: &ProjectDirs) -> Option<String> {
    let candidates = [
        dirs.user_preferences(),
        dirs.root.join("USER_PREFERENCES.md"),
    ];

    for path in &candidates {
        if let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            return Some(content.trim().to_string());
        }
    }

    None
}

fn check_version(dirs: &ProjectDirs) -> Option<String> {
    let version_file = dirs.version_file();
    if !version_file.exists() {
        return None;
    }

    let project_version = fs::read_to_string(&version_file).ok()?.trim().to_string();
    if project_version.is_empty() {
        return None;
    }

    let package_version = std::env::var("AMPLIHACK_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    if package_version == project_version {
        return None;
    }

    Some(format!(
        "⚠️ Version mismatch detected: package={package_version}, project={project_version}. Run `amplihack update` to update."
    ))
}

fn load_code_graph_context(dirs: &ProjectDirs) -> Option<String> {
    let db_path = resolve_code_graph_db_path_for_project(&dirs.root).ok()?;
    let summary = summarize_code_graph(Some(&db_path)).ok().flatten()?;
    let total = summary.files + summary.classes + summary.functions;
    if total == 0 {
        return None;
    }

    Some(format!(
        "## Code Graph (Blarify)\n\n\
         A code graph is available with {} files, {} classes, and {} functions indexed.\n\
         To query the code graph, use:\n\
         ```bash\n\
         amplihack query-code stats\n\
         amplihack query-code search <name>\n\
         amplihack query-code functions --file <path>\n\
         amplihack query-code classes --file <path>\n\
         amplihack query-code files --pattern <pattern>\n\
         amplihack query-code context <memory_id>\n\
         amplihack query-code callers <function_name>\n\
         amplihack query-code callees <function_name>\n\
         ```\n\
         Use `--json` for machine-readable output and `--limit N` to control result count.",
        summary.files, summary.classes, summary.functions
    ))
}

fn setup_blarify_indexing(dirs: &ProjectDirs) -> anyhow::Result<bool> {
    if std::env::var("AMPLIHACK_DISABLE_BLARIFY").as_deref() == Ok("1") {
        return Ok(false);
    }
    if background_index_job_active(&dirs.root)? {
        return Ok(true);
    }

    let status = check_index_status(&dirs.root)?;
    let db_path = resolve_code_graph_db_path_for_project(&dirs.root)?;
    let code_graph_missing = !db_path.exists();
    if !status.needs_indexing && !code_graph_missing {
        return Ok(false);
    }

    let action = resolve_blarify_index_action(&status, &blarify_json_path(&dirs.root));
    match blarify_mode() {
        SessionStartBlarifyMode::Skip => Ok(false),
        SessionStartBlarifyMode::Sync => {
            run_blarify_indexing(&dirs.root, action, false)?;
            Ok(false)
        }
        SessionStartBlarifyMode::Background => {
            run_blarify_indexing(&dirs.root, action, true)?;
            Ok(true)
        }
    }
}

fn run_blarify_indexing(
    project_root: &Path,
    action: BlarifyIndexAction,
    background: bool,
) -> anyhow::Result<()> {
    let amplihack = find_amplihack_binary()?;
    let mut cmd = build_blarify_index_command(&amplihack, project_root, action)?;
    cmd.current_dir(project_root);
    if background {
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        amplihack_cli::memory::record_background_index_pid(project_root, child.id())?;
    } else {
        let status = cmd.status()?;
        if !status.success() {
            anyhow::bail!("blarify indexing command failed with status {status}");
        }
    }
    Ok(())
}

fn build_blarify_index_command(
    amplihack_binary: &Path,
    project_root: &Path,
    action: BlarifyIndexAction,
) -> anyhow::Result<Command> {
    let mut cmd = Command::new(amplihack_binary);
    match action {
        BlarifyIndexAction::ImportExistingJson => {
            cmd.arg("index-code")
                .arg(blarify_json_path(project_root))
                .arg("--db-path")
                .arg(resolve_code_graph_db_path_for_project(project_root)?);
        }
        BlarifyIndexAction::GenerateNativeScip => {
            cmd.arg("index-scip")
                .arg("--project-path")
                .arg(project_root);
        }
    }
    Ok(cmd)
}

fn find_amplihack_binary() -> anyhow::Result<PathBuf> {
    Ok(BinaryFinder::find("amplihack")?.path)
}

fn blarify_json_path(project_root: &Path) -> PathBuf {
    project_root.join(".amplihack").join("blarify.json")
}

fn resolve_blarify_index_action(
    status: &amplihack_cli::memory::IndexStatus,
    json_path: &Path,
) -> BlarifyIndexAction {
    if json_path.exists() && !status.needs_indexing {
        BlarifyIndexAction::ImportExistingJson
    } else {
        BlarifyIndexAction::GenerateNativeScip
    }
}

fn blarify_mode() -> SessionStartBlarifyMode {
    match std::env::var("AMPLIHACK_BLARIFY_MODE")
        .unwrap_or_else(|_| "background".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "skip" => SessionStartBlarifyMode::Skip,
        "sync" => SessionStartBlarifyMode::Sync,
        _ => SessionStartBlarifyMode::Background,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlarifyIndexAction {
    ImportExistingJson,
    GenerateNativeScip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStartBlarifyMode {
    Skip,
    Sync,
    Background,
}

fn migrate_global_hooks() -> Option<String> {
    let global_settings = ProjectDirs::global_settings()?;
    if !global_settings.exists() {
        return None;
    }

    let settings_file = AtomicJsonFile::new(&global_settings);
    let settings: Value = match settings_file.read() {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("Failed to read global settings: {}", e);
            return Some(
                "⚠️ Global amplihack hooks may exist in ~/.claude/settings.json. \
                 Failed to read the file for migration."
                    .to_string(),
            );
        }
    };

    if !contains_amplihack_hooks(&settings) {
        return None;
    }

    match settings_file.update(|settings: &mut Value| remove_amplihack_hooks(settings)) {
        Ok(updated) if !contains_amplihack_hooks(&updated) => Some(
            "✅ Migrated amplihack hooks from global ~/.claude/settings.json to project-local hooks."
                .to_string(),
        ),
        Ok(_) => Some(
            "⚠️ Global amplihack hooks detected in ~/.claude/settings.json. \
             These should be migrated to project-local hooks."
                .to_string(),
        ),
        Err(e) => {
            tracing::warn!("Hook migration failed: {}", e);
            Some(
                "⚠️ Global amplihack hooks detected in ~/.claude/settings.json. \
                 Migration failed — please remove them manually."
                    .to_string(),
            )
        }
    }
}

fn contains_amplihack_hooks(settings: &Value) -> bool {
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .map(|hooks_map| {
            hooks_map.values().any(|wrappers| {
                wrappers
                    .as_array()
                    .is_some_and(|wrappers| wrappers.iter().any(wrapper_references_amplihack))
            })
        })
        .unwrap_or(false)
}

fn wrapper_references_amplihack(wrapper: &Value) -> bool {
    wrapper
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .map(|cmd| cmd.contains("amplihack-hooks") || cmd.contains("tools/amplihack/"))
                    .unwrap_or(false)
            })
        })
}

fn remove_amplihack_hooks(settings: &mut Value) {
    let Some(root) = settings.as_object_mut() else {
        *settings = serde_json::json!({});
        return;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };

    for wrappers in hooks.values_mut() {
        if let Some(wrappers) = wrappers.as_array_mut() {
            wrappers.retain(|wrapper| !wrapper_references_amplihack(wrapper));
        }
    }

    hooks.retain(|_, wrappers| {
        wrappers
            .as_array()
            .map(|arr| !arr.is_empty())
            .unwrap_or(true)
    });
}

#[cfg(test)]
fn generate_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("session-{}", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use amplihack_cli::commands::memory::code_graph::import_blarify_json;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    #[test]
    fn handles_unknown_events() {
        let hook = SessionStartHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn load_project_context_missing() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(load_project_context(&dirs).is_none());
    }

    #[test]
    fn load_project_context_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::write(dirs.root.join("PROJECT.md"), "# My Project\nDescription").unwrap();
        let ctx = load_project_context(&dirs);
        assert!(ctx.is_some());
        assert!(ctx.unwrap().contains("My Project"));
    }

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        assert!(id.starts_with("session-"));
    }

    #[test]
    fn check_version_returns_none_when_version_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(check_version(&dirs).is_none());
    }

    #[test]
    fn check_version_reports_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.claude).unwrap();
        fs::write(dirs.version_file(), "different-version\n").unwrap();

        let result = check_version(&dirs).expect("mismatch should be reported");
        assert!(result.contains("Version mismatch detected"));
        assert!(result.contains("different-version"));
    }

    #[test]
    fn load_code_graph_context_missing_db_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(load_code_graph_context(&dirs).is_none());
    }

    #[test]
    fn load_code_graph_context_describes_native_graph() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let input = dir.path().join("blarify.json");
        fs::write(
            &input,
            serde_json::json!({
                "files": [{
                    "id": "file:src/main.py",
                    "path": "src/main.py",
                    "language": "python",
                    "size_bytes": 12
                }],
                "classes": [{
                    "id": "class:Example",
                    "name": "Example",
                    "qualified_name": "pkg.Example",
                    "file_path": "src/main.py",
                    "line_number": 3
                }],
                "functions": [{
                    "id": "function:helper",
                    "name": "helper",
                    "qualified_name": "pkg.helper",
                    "signature": "helper()",
                    "file_path": "src/main.py",
                    "line_number": 8
                }],
                "imports": [],
                "relationships": []
            })
            .to_string(),
        )
        .unwrap();
        let db_path = resolve_code_graph_db_path_for_project(dir.path()).unwrap();
        import_blarify_json(&input, Some(&db_path)).unwrap();

        let context = load_code_graph_context(&dirs).expect("code graph context expected");
        assert!(context.contains("## Code Graph (Blarify)"));
        assert!(context.contains("1 files, 1 classes, and 1 functions"));
        assert!(context.contains("amplihack query-code stats"));
    }

    #[test]
    fn setup_blarify_indexing_background_imports_current_json_when_db_missing() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("app.py"), "print('hi')\n").unwrap();
        let artifact_dir = dir.path().join(".amplihack");
        fs::create_dir_all(&artifact_dir).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();

        let stub_log = dir.path().join("amplihack.log");
        let stub = dir.path().join("amplihack");
        fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo amplihack-test; exit 0; fi\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                stub_log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_BINARY_PATH", &stub);
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
        }

        let active = setup_blarify_indexing(&dirs).unwrap();

        let mut attempts = 0;
        while !stub_log.exists() && attempts < 20 {
            std::thread::sleep(Duration::from_millis(50));
            attempts += 1;
        }
        let logged = fs::read_to_string(&stub_log).unwrap();
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(active);
        assert!(logged.contains("index-code"));
        assert!(logged.contains(".amplihack/blarify.json"));
        assert!(logged.contains(".amplihack/kuzu_db"));
    }

    #[test]
    fn setup_blarify_indexing_sync_regenerates_stale_json_with_native_scip() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let artifact_dir = dir.path().join(".amplihack");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();
        std::thread::sleep(Duration::from_secs(1));
        fs::write(src_dir.join("app.py"), "print('updated')\n").unwrap();

        let stub_log = dir.path().join("amplihack-sync.log");
        let stub = dir.path().join("amplihack-sync");
        fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo amplihack-test; exit 0; fi\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                stub_log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_BINARY_PATH", &stub);
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "sync");
        }

        let active = setup_blarify_indexing(&dirs).unwrap();
        let logged = fs::read_to_string(&stub_log).unwrap();

        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(!active);
        assert!(logged.contains("index-scip"));
        assert!(logged.contains("--project-path"));
        assert!(logged.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn setup_blarify_indexing_reuses_existing_background_job() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        amplihack_cli::memory::record_background_index_pid(dir.path(), std::process::id()).unwrap();

        let active = setup_blarify_indexing(&dirs).unwrap();

        assert!(active);
    }

    #[test]
    fn remove_amplihack_hooks_preserves_third_party_entries() {
        let mut settings = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {"type": "command", "command": "/home/user/.local/bin/amplihack-hooks session-start"}
                        ]
                    },
                    {
                        "hooks": [
                            {"type": "command", "command": "/usr/local/bin/third-party-hook"}
                        ]
                    }
                ],
                "UserPromptSubmit": [
                    {
                        "hooks": [
                            {"type": "command", "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/user_prompt_submit.py"}
                        ]
                    }
                ]
            }
        });

        remove_amplihack_hooks(&mut settings);

        assert!(!contains_amplihack_hooks(&settings));
        let session_wrappers = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_wrappers.len(), 1);
        assert_eq!(
            session_wrappers[0]["hooks"][0]["command"].as_str(),
            Some("/usr/local/bin/third-party-hook")
        );
        assert!(settings["hooks"].get("UserPromptSubmit").is_none());
    }

    #[test]
    fn migrate_global_hooks_updates_settings_atomically() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", dir.path()) };

        let settings_path = dir.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "SessionStart": [
                        {
                            "hooks": [
                                {"type": "command", "command": "/home/user/.local/bin/amplihack-hooks session-start"}
                            ]
                        },
                        {
                            "hooks": [
                                {"type": "command", "command": "/usr/local/bin/third-party-hook"}
                            ]
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let message = migrate_global_hooks().expect("migration message expected");

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(message.contains("Migrated amplihack hooks"));
        let updated: Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(!contains_amplihack_hooks(&updated));
        assert_eq!(
            updated["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str(),
            Some("/usr/local/bin/third-party-hook")
        );
    }
}
