//! Session start hook: initializes session state and injects context.
//!
//! On session start, this hook:
//! 1. Checks for version mismatches
//! 2. Migrates global hooks if needed
//! 3. Captures original request
//! 4. Injects project context, learnings, and preferences
//! 5. Returns additional context for the session

use crate::original_request::{capture_original_request, format_original_request_context};
use crate::protocol::{FailurePolicy, Hook};
use amplihack_cli::binary_finder::BinaryFinder;
use amplihack_cli::memory::{
    background_index_job_active, check_index_status, default_code_graph_db_path_for_project,
    resolve_code_graph_db_path_for_project, summarize_code_graph,
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
        let (session_id, extra) = match input {
            HookInput::SessionStart {
                session_id, extra, ..
            } => (session_id, extra),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let dirs = ProjectDirs::from_cwd();
        let mut context_parts: Vec<String> = Vec::new();
        // Warnings accumulate structured failures for the HookOutput `warnings` field.
        // These surface errors that did not block session start (fail-open) but that
        // the host should be aware of — e.g. code-graph setup failures.
        let mut warnings: Vec<String> = Vec::new();

        if let Some(original_request_context) =
            maybe_capture_original_request(&dirs, session_id.as_deref(), &extra)?
        {
            context_parts.push(original_request_context);
        }

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

        context_parts.push(load_workflow_context(&dirs));

        // Check for version mismatch natively.
        if let Some(version_notice) = check_version(&dirs) {
            context_parts.push(version_notice);
        }

        // Migrate global hooks if needed.
        if let Some(migration_notice) = migrate_global_hooks() {
            context_parts.push(migration_notice);
        }

        // Run blarify / code-graph indexing setup and track the lifecycle status.
        //
        // indexing_status values (parity with amploxy SessionStart):
        //   "started"       — background indexing was triggered or was already running
        //   "complete"      — index is up-to-date; no new indexing triggered
        //   "error:<reason>" — setup failed; session continues (fail-open)
        let (indexing_status, blarify_setup) = match setup_blarify_indexing(&dirs) {
            Ok(result) => {
                let status = if result.indexing_active {
                    "started".to_string()
                } else {
                    "complete".to_string()
                };
                (status, result)
            }
            Err(err) => {
                let error_msg = format!("Code-graph setup failed: {err}");
                tracing::warn!("Blarify setup failed (non-critical): {}", err);
                // Surface the failure as a structured warning — not just buried in text.
                warnings.push(error_msg.clone());
                let notice = BlarifySetupResult::with_notice(
                    false,
                    format_code_graph_status(format!(
                        "{error_msg}. Continuing without automatic refresh."
                    )),
                );
                (format!("error:{err}"), notice)
            }
        };

        if let Some(status_context) = blarify_setup.status_context {
            context_parts.push(status_context);
        }
        if let Some(compatibility_notice) = code_graph_compatibility_notice(&dirs)? {
            context_parts.push(compatibility_notice);
        }
        if let Some(memory_notice) = memory_graph_compatibility_notice() {
            context_parts.push(memory_notice);
        }

        if !blarify_setup.indexing_active {
            match load_code_graph_context(&dirs) {
                Ok(Some(code_graph_context)) => context_parts.push(code_graph_context),
                Ok(None) => {}
                Err(err) => {
                    let error_msg = format!("Code-graph context unavailable: {err}");
                    warnings.push(error_msg.clone());
                    context_parts.push(format_code_graph_status(error_msg));
                }
            }
        }

        let additional_context = context_parts.join("\n\n");

        // Always emit hookSpecificOutput so that `indexing_status` is never absent.
        // When there is no additionalContext, we still report the indexing lifecycle.
        let mut hook_specific = serde_json::json!({
            "hookEventName": "SessionStart",
            "indexing_status": indexing_status,
        });
        if !additional_context.is_empty() {
            hook_specific["additionalContext"] = Value::String(additional_context);
        }

        let mut output = serde_json::json!({
            "hookSpecificOutput": hook_specific,
        });
        if !warnings.is_empty() {
            output["warnings"] = serde_json::json!(warnings);
        }

        Ok(output)
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
    let mut candidates = Vec::new();
    if let Some(path) = dirs.resolve_preferences_file() {
        candidates.push(path);
    }
    candidates.push(dirs.root.join("USER_PREFERENCES.md"));

    for path in &candidates {
        if let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            return Some(content.trim().to_string());
        }
    }

    None
}

fn load_workflow_context(dirs: &ProjectDirs) -> String {
    let mut parts = vec![
        "## Default Workflow".to_string(),
        "The multi-step workflow is automatically followed by `/ultrathink`".to_string(),
    ];

    if let Some(path) = dirs.resolve_workflow_file() {
        parts.push(format!("• To view the workflow: Read {}", path.display()));
        parts.push("• To customize: Edit the workflow file directly".to_string());
    } else {
        parts.push("• To view the workflow: Read .claude/workflow/DEFAULT_WORKFLOW.md".to_string());
        parts.push("• To customize: Edit the workflow file directly".to_string());
    }

    parts.push(
        "• Steps include: Requirements → Issue → Branch → Design → Implement → Review → Merge"
            .to_string(),
    );

    parts.join("\n")
}

fn maybe_capture_original_request(
    dirs: &ProjectDirs,
    session_id: Option<&str>,
    extra: &Value,
) -> anyhow::Result<Option<String>> {
    let Some(prompt) = extra.get("prompt").and_then(Value::as_str).map(str::trim) else {
        return Ok(None);
    };

    Ok(capture_original_request(dirs, session_id, prompt)?
        .as_ref()
        .map(format_original_request_context))
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

fn load_code_graph_context(dirs: &ProjectDirs) -> anyhow::Result<Option<String>> {
    let db_path = resolve_code_graph_db_path_for_project(&dirs.root)?;
    let Some(summary) = summarize_code_graph(Some(&db_path))? else {
        return Ok(None);
    };
    let total = summary.files + summary.classes + summary.functions;
    if total == 0 {
        return Ok(None);
    }

    Ok(Some(format!(
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
    )))
}

fn code_graph_compatibility_notice(dirs: &ProjectDirs) -> anyhow::Result<Option<String>> {
    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(None);
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(Some(format_code_graph_status(
            "Using legacy `AMPLIHACK_KUZU_DB_PATH` compatibility alias for the code graph. Prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        )));
    }

    let neutral = default_code_graph_db_path_for_project(&dirs.root)?;
    let legacy = dirs.root.join(".amplihack").join("kuzu_db");
    if legacy.exists() && !neutral.exists() {
        return Ok(Some(format_code_graph_status(format!(
            "Using legacy code-graph store `{}` because `{}` is absent. Migrate to the neutral `graph_db` path to leave compatibility mode.",
            legacy.display(),
            neutral.display()
        ))));
    }

    Ok(None)
}

fn memory_graph_compatibility_notice() -> Option<String> {
    if std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() == Some("sqlite") {
        return None;
    }

    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Some(format_memory_status(
            "Using legacy `AMPLIHACK_KUZU_DB_PATH` compatibility alias for the memory graph. Prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        ));
    }

    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let neutral = home.join(".amplihack").join("memory_graph.db");
    let legacy = home.join(".amplihack").join("memory_kuzu.db");
    if legacy.exists() && !neutral.exists() {
        return Some(format_memory_status(format!(
            "Using legacy memory graph store `{}` because `{}` is absent. Migrate to `memory_graph.db` to leave compatibility mode.",
            legacy.display(),
            neutral.display()
        )));
    }

    None
}

fn setup_blarify_indexing(dirs: &ProjectDirs) -> anyhow::Result<BlarifySetupResult> {
    if background_index_job_active(&dirs.root)? {
        return Ok(BlarifySetupResult::with_notice(
            true,
            format_code_graph_status(
                "A background code-graph refresh is already running for this project. \
                 The code graph may be unavailable or locked until it finishes. \
                 Retry `amplihack query-code stats` after it completes."
                    .to_string(),
            ),
        ));
    }

    let status = check_index_status(&dirs.root)?;
    let db_path = resolve_code_graph_db_path_for_project(&dirs.root)?;
    let code_graph_missing = !db_path.exists();
    let needs_setup = status.needs_indexing || code_graph_missing;
    if !needs_setup {
        return Ok(BlarifySetupResult::ready());
    }

    if std::env::var("AMPLIHACK_DISABLE_BLARIFY").as_deref() == Ok("1") {
        return Ok(BlarifySetupResult::with_notice(
            false,
            format_code_graph_status(format!(
                "Automatic code-graph refresh is disabled by `AMPLIHACK_DISABLE_BLARIFY=1`, \
                 but setup is still needed because {}. The current code graph may be missing or stale.",
                describe_blarify_need(&status, code_graph_missing)
            )),
        ));
    }

    if !status.needs_indexing && !code_graph_missing {
        return Ok(BlarifySetupResult::ready());
    }

    let action = resolve_blarify_index_action(&status, &blarify_json_path(&dirs.root));
    match blarify_mode() {
        SessionStartBlarifyMode::Skip => Ok(BlarifySetupResult::with_notice(
            false,
            format_code_graph_status(format!(
                "Code-graph setup was needed because {}, but `AMPLIHACK_BLARIFY_MODE=skip` \
                 prevented it from running. Skipped action: {}. The current code graph may be missing or stale.",
                describe_blarify_need(&status, code_graph_missing),
                describe_blarify_action(action)
            )),
        )),
        SessionStartBlarifyMode::Sync => {
            run_blarify_indexing(&dirs.root, action, false, &db_path)?;
            Ok(BlarifySetupResult::ready())
        }
        SessionStartBlarifyMode::Background => {
            run_blarify_indexing(&dirs.root, action, true, &db_path)?;
            Ok(BlarifySetupResult::with_notice(
                true,
                format_code_graph_status(format!(
                    "Started background code-graph setup because {}. Planned action: {}. \
                     The code graph may be unavailable or locked until it finishes. \
                     Retry `amplihack query-code stats` after it completes.",
                    describe_blarify_need(&status, code_graph_missing),
                    describe_blarify_action(action)
                )),
            ))
        }
    }
}

fn run_blarify_indexing(
    project_root: &Path,
    action: BlarifyIndexAction,
    background: bool,
    db_path: &Path,
) -> anyhow::Result<()> {
    let amplihack = find_amplihack_binary()?;
    let mut cmd = build_blarify_index_command(&amplihack, project_root, action, db_path)?;
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
    db_path: &Path,
) -> anyhow::Result<Command> {
    let mut cmd = Command::new(amplihack_binary);
    match action {
        BlarifyIndexAction::ImportExistingJson => {
            cmd.arg("index-code")
                .arg(blarify_json_path(project_root))
                .arg("--db-path")
                .arg(db_path);
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlarifySetupResult {
    indexing_active: bool,
    status_context: Option<String>,
}

impl BlarifySetupResult {
    fn ready() -> Self {
        Self {
            indexing_active: false,
            status_context: None,
        }
    }

    fn with_notice(indexing_active: bool, status_context: String) -> Self {
        Self {
            indexing_active,
            status_context: Some(status_context),
        }
    }
}

fn format_code_graph_status(body: String) -> String {
    format!("## Code Graph Status\n\n{body}")
}

fn format_memory_status(body: String) -> String {
    format!("## Memory Store Status\n\n{body}")
}

fn describe_blarify_need(
    status: &amplihack_cli::memory::IndexStatus,
    code_graph_missing: bool,
) -> String {
    match (status.needs_indexing, code_graph_missing) {
        (true, true) => format!(
            "{} and the project code-graph database is missing",
            status.reason
        ),
        (true, false) => status.reason.clone(),
        (false, true) => "the project code-graph database is missing".to_string(),
        (false, false) => "no refresh is required".to_string(),
    }
}

fn describe_blarify_action(action: BlarifyIndexAction) -> &'static str {
    match action {
        BlarifyIndexAction::ImportExistingJson => {
            "import the current Blarify JSON into the project code-graph database"
        }
        BlarifyIndexAction::GenerateNativeScip => {
            "rebuild the project code graph with native SCIP indexing"
        }
    }
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
    fn load_workflow_context_uses_amplihack_root_override() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = tempfile::tempdir().unwrap();
        let framework = tempfile::tempdir().unwrap();
        fs::create_dir_all(framework.path().join(".claude/workflow")).unwrap();
        fs::write(
            framework
                .path()
                .join(".claude/workflow/DEFAULT_WORKFLOW.md"),
            "# Default Workflow\n",
        )
        .unwrap();
        let previous = std::env::var_os("AMPLIHACK_ROOT");
        unsafe { std::env::set_var("AMPLIHACK_ROOT", framework.path()) };

        let context = load_workflow_context(&ProjectDirs::new(project.path()));

        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
        }

        assert!(context.contains("## Default Workflow"));
        assert!(
            context.contains(
                framework
                    .path()
                    .join(".claude/workflow/DEFAULT_WORKFLOW.md")
                    .display()
                    .to_string()
                    .as_str()
            )
        );
    }

    #[test]
    fn session_start_captures_original_request_context() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip") };

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: None,
                extra: serde_json::json!({
                    "prompt": "Implement complete hook parity. Do not regress tests. Ensure every user-visible hook output matches Python."
                }),
            })
            .unwrap();

        unsafe { std::env::remove_var("AMPLIHACK_BLARIFY_MODE") };
        let _ = std::env::set_current_dir(&original);

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("## 🎯 ORIGINAL USER REQUEST - PRESERVE THESE REQUIREMENTS"));
        assert!(context.contains("**Constraints**:"));
        assert!(context.contains("**Success Criteria**:"));
        assert!(
            dir.path()
                .join(".claude/runtime/logs/test-session/ORIGINAL_REQUEST.md")
                .exists()
        );
        assert!(
            dir.path()
                .join(".claude/runtime/logs/test-session/original_request.json")
                .exists()
        );
    }

    #[test]
    fn load_code_graph_context_missing_db_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(load_code_graph_context(&dirs).unwrap().is_none());
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

        let context = load_code_graph_context(&dirs)
            .unwrap()
            .expect("code graph context expected");
        assert!(context.contains("## Code Graph (Blarify)"));
        assert!(context.contains("1 files, 1 classes, and 1 functions"));
        assert!(context.contains("amplihack query-code stats"));
    }

    #[test]
    fn session_start_process_surfaces_code_graph_context_failure_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let broken_db = dir.path().join("broken-graph-db");
        fs::write(&broken_db, "not a graph db").unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &broken_db);
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context");
        assert!(context.contains("## Code Graph Status"));
        assert!(context.contains("Code-graph context unavailable"));

        let warnings = result["warnings"]
            .as_array()
            .expect("warnings array expected");
        assert!(warnings.iter().any(|warning| {
            warning
                .as_str()
                .unwrap_or("")
                .contains("Code-graph context unavailable")
        }));
    }

    #[test]
    fn session_start_process_surfaces_legacy_graph_env_alias_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let legacy_override = dir.path().join("legacy-graph-db");
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &legacy_override);
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context");
        assert!(context.contains("## Code Graph Status"));
        assert!(context.contains("AMPLIHACK_KUZU_DB_PATH"));
        assert!(context.contains("AMPLIHACK_GRAPH_DB_PATH"));
    }

    #[test]
    fn session_start_process_surfaces_legacy_graph_store_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let legacy_store = dir.path().join(".amplihack").join("kuzu_db");
        fs::create_dir_all(&legacy_store).unwrap();
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context");
        assert!(context.contains("## Code Graph Status"));
        assert!(context.contains(".amplihack/kuzu_db"));
        assert!(context.contains("graph_db"));
    }

    #[test]
    fn session_start_process_surfaces_legacy_memory_env_alias_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let legacy_override = dir.path().join("legacy-memory-db");
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &legacy_override);
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context");
        assert!(context.contains("## Memory Store Status"));
        assert!(context.contains("AMPLIHACK_KUZU_DB_PATH"));
        assert!(context.contains("AMPLIHACK_GRAPH_DB_PATH"));
    }

    #[test]
    fn session_start_process_surfaces_legacy_memory_store_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let legacy_store = home.path().join(".amplihack").join("memory_kuzu.db");
        fs::create_dir_all(legacy_store.parent().unwrap()).unwrap();
        fs::write(&legacy_store, "legacy-memory").unwrap();
        let previous_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context");
        assert!(context.contains("## Memory Store Status"));
        assert!(context.contains("memory_kuzu.db"));
        assert!(context.contains("memory_graph.db"));
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

        let result = setup_blarify_indexing(&dirs).unwrap();

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

        assert!(result.indexing_active);
        assert!(
            result
                .status_context
                .as_deref()
                .is_some_and(|context| context.contains("Started background code-graph setup"))
        );
        assert!(logged.contains("index-code"));
        assert!(logged.contains(".amplihack/blarify.json"));
        assert!(logged.contains(".amplihack/graph_db"));
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

        let result = setup_blarify_indexing(&dirs).unwrap();
        let logged = fs::read_to_string(&stub_log).unwrap();

        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(!result.indexing_active);
        assert!(result.status_context.is_none());
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

        let result = setup_blarify_indexing(&dirs).unwrap();

        assert!(result.indexing_active);
        assert!(
            result
                .status_context
                .as_deref()
                .is_some_and(|context| context.contains("already running"))
        );
    }

    #[test]
    fn setup_blarify_indexing_skip_surfaces_status_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let result = setup_blarify_indexing(&dirs).unwrap();

        unsafe {
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(!result.indexing_active);
        let context = result.status_context.expect("skip notice expected");
        assert!(context.contains("## Code Graph Status"));
        assert!(context.contains("AMPLIHACK_BLARIFY_MODE=skip"));
        assert!(context.contains("missing or stale"));
    }

    #[test]
    fn session_start_process_surfaces_blarify_setup_failure_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        unsafe {
            std::env::set_var(
                "AMPLIHACK_AMPLIHACK_BINARY_PATH",
                dir.path().join("missing-amplihack"),
            );
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("test-session".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context");
        assert!(context.contains("## Code Graph Status"));
        assert!(context.contains("Code-graph setup failed"));

        // AC: When the code-graph setup (binary lookup) fails, HookOutput must contain
        // a non-empty `warnings` field — the failure must not be silently swallowed.
        let warnings = result["warnings"]
            .as_array()
            .expect("HookOutput must have a 'warnings' array when blarify setup fails");
        assert!(
            !warnings.is_empty(),
            "warnings array must be non-empty on setup failure"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.as_str().unwrap_or("").contains("Code-graph setup failed")),
            "at least one warning must mention the setup failure"
        );

        // AC: indexing_status must be present and carry an error value.
        let status = result["hookSpecificOutput"]["indexing_status"]
            .as_str()
            .expect("indexing_status must be present in hookSpecificOutput");
        assert!(
            status.starts_with("error:"),
            "indexing_status must start with 'error:' on setup failure, got: {status}"
        );
    }

    #[test]
    fn session_start_process_always_emits_indexing_status() {
        // AC: indexing_status must be present in hookSpecificOutput even when
        // there is no additionalContext (empty project directory).
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        // Disable blarify so we get a deterministic "complete" status.
        unsafe {
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let hook = SessionStartHook;
        let result = hook
            .process(HookInput::SessionStart {
                session_id: Some("status-test".to_string()),
                cwd: Some(dir.path().to_path_buf()),
                extra: Value::Object(serde_json::Map::new()),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);
        unsafe {
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let status = result["hookSpecificOutput"]["indexing_status"]
            .as_str()
            .expect("indexing_status must always be present in hookSpecificOutput");
        assert!(
            status == "started" || status == "complete" || status.starts_with("error:"),
            "indexing_status must be 'started', 'complete', or 'error:<reason>', got: {status}"
        );
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
