//! Context loading functions for session start.

use amplihack_cli::memory::{resolve_code_graph_db_path_for_project, summarize_code_graph};
use amplihack_types::ProjectDirs;
use std::fs;

pub(super) fn load_project_context(dirs: &ProjectDirs) -> Option<String> {
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

pub(super) fn load_discoveries(dirs: &ProjectDirs) -> Option<String> {
    let path = dirs.root.join("DISCOVERIES.md");
    if let Ok(content) = fs::read_to_string(path)
        && !content.trim().is_empty()
    {
        return Some(format!("## Recent Learnings\n\n{}", content.trim()));
    }
    None
}

pub(super) fn load_user_preferences(dirs: &ProjectDirs) -> Option<String> {
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

pub(super) fn load_workflow_context(dirs: &ProjectDirs) -> String {
    // Nested recipe sessions should not get top-level /dev rules to prevent
    // recursive workflow invocation (ported from Python PR #4142).
    if is_nested_recipe_session() || is_workflow_active(dirs) {
        return build_suppressed_workflow_rules();
    }

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

pub(super) fn check_version(dirs: &ProjectDirs) -> Option<String> {
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

pub(super) fn load_code_graph_context(dirs: &ProjectDirs) -> anyhow::Result<Option<String>> {
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

/// Return `true` when a nested session is running inside a recipe.
pub(crate) fn is_nested_recipe_session() -> bool {
    std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
        > 0
}

/// Return `true` when a workflow-active semaphore points at a live process.
pub(crate) fn is_workflow_active(dirs: &ProjectDirs) -> bool {
    let path = dirs.runtime.join("locks").join(".workflow_active");
    if !path.exists() {
        return false;
    }

    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(_) => {
            let _ = fs::remove_file(&path);
            return false;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => {
            let _ = fs::remove_file(&path);
            return false;
        }
    };

    let pid = parsed.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    if pid == 0 {
        let _ = fs::remove_file(&path);
        return false;
    }

    // Check if PID is alive via /proc on Linux, or assume alive otherwise.
    #[cfg(target_os = "linux")]
    {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            let _ = fs::remove_file(&path);
            return false;
        }
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Cannot reliably check PID without libc; assume alive.
        true
    }
}

/// Return nested-session guidance that prevents workflow recursion.
fn build_suppressed_workflow_rules() -> String {
    "## Default Workflow\n\n\
     A recipe-managed workflow is already active for this session.\n\n\
     Do NOT invoke `Skill(skill=\"dev-orchestrator\")`, do NOT run \
     `run_recipe_by_name(\"smart-orchestrator\")`, and do NOT reinterpret \
     the current prompt as a new top-level task.\n\n\
     Follow the current prompt exactly. Return only the requested output \
     format. Use tools only when the prompt explicitly requires them."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use amplihack_cli::commands::memory::code_graph::import_blarify_json;
    use amplihack_cli::memory::resolve_code_graph_db_path_for_project;
    use std::fs;

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
}
