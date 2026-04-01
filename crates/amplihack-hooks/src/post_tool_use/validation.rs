//! Tool result validation and blarify index staleness detection.
//!
//! Validates Write/Edit/MultiEdit results and marks the blarify index as stale
//! when code files are modified.

use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Code-file extension set (mirrors blarify_staleness_hook.py)
// ---------------------------------------------------------------------------

const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".js", ".jsx", ".ts", ".tsx", ".cs", ".go", ".rs", ".c", ".h", ".cpp", ".hpp", ".cc",
    ".cxx", ".java", ".php", ".rb",
];

/// Return `true` if `path` has a code-file extension.
pub(super) fn is_code_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    CODE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate tool-specific results for Write/Edit/MultiEdit.
///
/// Returns a warning string if the tool result indicates a problem.
pub(super) fn validate_tool_result(tool_name: &str, tool_result: Option<&Value>) -> Option<String> {
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
pub(super) fn extract_written_paths(tool_name: &str, tool_input: &Value) -> Vec<String> {
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
pub(super) fn mark_blarify_stale_if_needed(tool_name: &str, tool_input: &Value) {
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
