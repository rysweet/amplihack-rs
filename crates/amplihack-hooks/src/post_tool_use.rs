//! Post-tool-use hook: observe tool results, validate operations, and detect
//! blarify index staleness.
//!
//! # Responsibilities
//!
//! 1. **Metrics**: Records tool invocation metrics (tool name, category,
//!    timestamp) to a JSONL file for later analysis.
//! 2. **Validation**: Performs tool-specific result validation for
//!    Write/Edit/MultiEdit operations and emits warnings on failure.
//! 3. **Blarify staleness** (parity with `blarify_staleness_hook.py`):
//!    When a code file is modified via Write/Edit/MultiEdit the hook writes a
//!    `.amplihack/blarify_stale` marker file so that the next session start (or
//!    explicit `amplihack index-code`) knows to trigger a re-index.
//!
//! None of these operations block the tool — failure policy is `Open`.

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use amplihack_types::ProjectDirs;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::SystemTime;

pub struct PostToolUseHook;

// ---------------------------------------------------------------------------
// Code-file extension set (mirrors blarify_staleness_hook.py)
// ---------------------------------------------------------------------------

const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".js", ".jsx", ".ts", ".tsx", ".cs", ".go", ".rs", ".c", ".h", ".cpp", ".hpp", ".cc",
    ".cxx", ".java", ".php", ".rb",
];

/// Return `true` if `path` has a code-file extension.
fn is_code_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    CODE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

// ---------------------------------------------------------------------------
// Tool categorisation
// ---------------------------------------------------------------------------

/// Categorize a tool invocation for high-level metrics.
fn categorize_tool(name: &str) -> &'static str {
    match name {
        "Bash" | "bash" | "terminal" => "bash_commands",
        "Write" | "Edit" | "MultiEdit" | "create" | "edit" => "file_operations",
        "Read" | "View" | "view" | "glob" | "grep" | "Grep" | "Search" => "search_operations",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate tool-specific results for Write/Edit/MultiEdit.
///
/// Returns a warning string if the tool result indicates a problem.
fn validate_tool_result(tool_name: &str, tool_result: Option<&Value>) -> Option<String> {
    let result = tool_result?;

    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "create" | "edit" => {
            // Check for error indicators in the result.
            if let Some(error) = result.get("error").and_then(Value::as_str) {
                return Some(format!("{tool_name} error: {error}"));
            }
            if let Some(false) = result.get("success").and_then(Value::as_bool) {
                let msg = result
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Some(format!("{tool_name} failed: {msg}"));
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Blarify staleness
// ---------------------------------------------------------------------------

/// Extract one or more file paths written by the tool from its `tool_input`.
///
/// Handles:
/// - `Write`/`create`: `input.path` (string)
/// - `Edit`/`edit`:    `input.file_path` or `input.path` (string)
/// - `MultiEdit`:      `input.edits[*].file_path` (array)
fn extract_written_paths(tool_name: &str, tool_input: &Value) -> Vec<String> {
    let mut paths = Vec::new();

    match tool_name {
        "Write" | "create" => {
            if let Some(p) = tool_input.get("path").and_then(Value::as_str) {
                paths.push(p.to_string());
            }
        }
        "Edit" | "edit" => {
            for key in &["file_path", "path"] {
                if let Some(p) = tool_input.get(key).and_then(Value::as_str) {
                    paths.push(p.to_string());
                    break;
                }
            }
        }
        "MultiEdit" => {
            if let Some(edits) = tool_input.get("edits").and_then(Value::as_array) {
                for edit in edits {
                    if let Some(p) = edit.get("file_path").and_then(Value::as_str) {
                        paths.push(p.to_string());
                    }
                }
            }
        }
        _ => {}
    }

    paths
}

/// Write a staleness marker if any modified path is a code file.
///
/// The marker is `.amplihack/blarify_stale` relative to the project root
/// (`ProjectDirs::from_cwd()`).  The content is a JSON object recording the
/// modified file path and timestamp so operators can correlate what triggered
/// the staleness.
fn mark_blarify_stale_if_needed(tool_name: &str, tool_input: &Value) {
    let paths = extract_written_paths(tool_name, tool_input);
    let code_path = paths.iter().find(|p| is_code_file(p));
    let Some(path) = code_path else {
        return;
    };

    let dirs = ProjectDirs::from_cwd();
    // The marker lives at <project_root>/.amplihack/blarify_stale
    let marker = dirs.root.join(".amplihack").join("blarify_stale");

    if let Err(e) = fs::create_dir_all(marker.parent().expect("marker has parent")) {
        tracing::warn!("blarify staleness: failed to create dir: {}", e);
        return;
    }

    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let content = serde_json::json!({
        "stale": true,
        "reason": "code_file_modified",
        "path": path,
        "tool": tool_name,
        "timestamp": ts,
    });

    if let Err(e) = fs::write(&marker, content.to_string()) {
        tracing::warn!("blarify staleness: failed to write marker: {}", e);
    } else {
        tracing::debug!(
            "blarify staleness marker written (tool={}, path={})",
            tool_name,
            path
        );
    }
}

// ---------------------------------------------------------------------------
// Hook implementation
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ToolMetric {
    timestamp: String,
    tool_name: String,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    hook: &'static str,
}

impl Hook for PostToolUseHook {
    fn name(&self) -> &'static str {
        "post_tool_use"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (tool_name, tool_input, tool_result, session_id) = match input {
            HookInput::PostToolUse {
                tool_name,
                tool_input,
                tool_result,
                session_id,
            } => (tool_name, tool_input, tool_result, session_id),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // Tool-specific validation.
        let warning = validate_tool_result(&tool_name, tool_result.as_ref());
        if let Some(ref w) = warning {
            tracing::warn!("{}", w);
        }

        // Blarify staleness detection (parity with blarify_staleness_hook.py).
        mark_blarify_stale_if_needed(&tool_name, &tool_input);

        // Record the tool metric with category.
        if let Err(e) = save_tool_metric(&tool_name, session_id.as_deref(), warning.as_deref()) {
            tracing::warn!("Failed to save tool metric: {}", e);
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

fn save_tool_metric(
    tool_name: &str,
    session_id: Option<&str>,
    warning: Option<&str>,
) -> anyhow::Result<()> {
    let dirs = ProjectDirs::from_cwd();
    fs::create_dir_all(&dirs.metrics)?;

    let metrics_file = dirs.metrics.join("post_tool_use_metrics.jsonl");

    let metric = ToolMetric {
        timestamp: now_iso8601(),
        tool_name: tool_name.to_string(),
        category: categorize_tool(tool_name),
        session_id: session_id.map(String::from),
        warning: warning.map(String::from),
        hook: "post_tool_use",
    };

    let json = serde_json::to_string(&metric)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(metrics_file)?;
    writeln!(file, "{}", json)?;

    Ok(())
}

fn now_iso8601() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_tools_correctly() {
        assert_eq!(categorize_tool("Bash"), "bash_commands");
        assert_eq!(categorize_tool("Write"), "file_operations");
        assert_eq!(categorize_tool("Edit"), "file_operations");
        assert_eq!(categorize_tool("grep"), "search_operations");
        assert_eq!(categorize_tool("CustomTool"), "other");
    }

    #[test]
    fn validates_edit_errors() {
        let result = serde_json::json!({"error": "file not found"});
        let warning = validate_tool_result("Edit", Some(&result));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("file not found"));
    }

    #[test]
    fn validates_success_false() {
        let result = serde_json::json!({"success": false, "message": "permission denied"});
        let warning = validate_tool_result("Write", Some(&result));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("permission denied"));
    }

    #[test]
    fn no_warning_on_success() {
        let result = serde_json::json!({"success": true});
        assert!(validate_tool_result("Edit", Some(&result)).is_none());
    }

    #[test]
    fn no_warning_for_bash() {
        let result = serde_json::json!({"error": "something"});
        assert!(validate_tool_result("Bash", Some(&result)).is_none());
    }

    #[test]
    fn is_code_file_detects_known_extensions() {
        assert!(is_code_file("src/main.rs"));
        assert!(is_code_file("app/module.py"));
        assert!(is_code_file("index.ts"));
        assert!(is_code_file("Component.tsx"));
        assert!(!is_code_file("README.md"));
        assert!(!is_code_file("config.yaml"));
        assert!(!is_code_file("image.png"));
    }

    #[test]
    fn is_code_file_case_insensitive() {
        assert!(is_code_file("Main.RS"));
        assert!(is_code_file("App.PY"));
    }

    #[test]
    fn extract_written_paths_write_tool() {
        let input = serde_json::json!({"path": "src/main.rs", "content": "fn main() {}"});
        let paths = extract_written_paths("Write", &input);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn extract_written_paths_edit_tool_file_path() {
        let input =
            serde_json::json!({"file_path": "src/lib.rs", "old_string": "a", "new_string": "b"});
        let paths = extract_written_paths("Edit", &input);
        assert_eq!(paths, vec!["src/lib.rs"]);
    }

    #[test]
    fn extract_written_paths_multiedit_tool() {
        let input = serde_json::json!({
            "edits": [
                {"file_path": "src/a.rs", "old_string": "a", "new_string": "b"},
                {"file_path": "src/b.rs", "old_string": "c", "new_string": "d"},
            ]
        });
        let paths = extract_written_paths("MultiEdit", &input);
        assert_eq!(paths, vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn extract_written_paths_bash_returns_empty() {
        let input = serde_json::json!({"command": "ls"});
        let paths = extract_written_paths("Bash", &input);
        assert!(paths.is_empty());
    }

    #[test]
    fn blarify_stale_marker_written_for_code_file_edit() {
        let dir = tempfile::tempdir().unwrap();
        // Temporarily change cwd for ProjectDirs resolution.
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());

        let input = serde_json::json!({
            "file_path": "src/main.rs",
            "old_string": "foo",
            "new_string": "bar",
        });
        mark_blarify_stale_if_needed("Edit", &input);

        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }

        let marker = dir.path().join(".amplihack").join("blarify_stale");
        assert!(marker.exists(), "blarify_stale marker should be written");
        let content = fs::read_to_string(&marker).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["stale"], true);
        assert_eq!(parsed["tool"], "Edit");
        assert_eq!(parsed["reason"], "code_file_modified");
    }

    #[test]
    fn blarify_stale_marker_not_written_for_non_code_file() {
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());

        let input = serde_json::json!({
            "file_path": "docs/README.md",
            "old_string": "a",
            "new_string": "b",
        });
        mark_blarify_stale_if_needed("Edit", &input);

        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }

        let marker = dir.path().join(".amplihack").join("blarify_stale");
        assert!(
            !marker.exists(),
            "blarify_stale marker should NOT be written for non-code files"
        );
    }

    #[test]
    fn allows_all_tools() {
        let hook = PostToolUseHook;
        let input = HookInput::PostToolUse {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({"command": "ls"}),
            tool_result: None,
            session_id: None,
        };
        let result = hook.process(input).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn handles_unknown_events() {
        let hook = PostToolUseHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }
}
