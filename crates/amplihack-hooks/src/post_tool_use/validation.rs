//! Tool result validation and blarify index staleness detection.
//!
//! Validates Write/Edit/MultiEdit results and marks the blarify index as stale
//! when code files are modified.

use crate::state_dir::create_private_dir_all;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
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

/// The project key: the first 16 lowercase hex digits of the SHA-256 of the
/// canonicalised project path, or of the path as given if it cannot be
/// canonicalised. The path itself never appears in a path component.
fn project_key(project_root: &Path) -> String {
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `<state root>/runtime/projects/<project key>/blarify_stale` (#1963).
///
/// The marker describes the project's code graph, so it is keyed by project
/// rather than by session.
pub(super) fn blarify_marker_path(state_root: &Path, project_root: &Path) -> PathBuf {
    state_root
        .join("runtime")
        .join("projects")
        .join(project_key(project_root))
        .join("blarify_stale")
}

/// Write a staleness marker if any modified path is a code file.
///
/// The marker lives under the amplihack state root at the path given by
/// [`blarify_marker_path`] for the current working directory; nothing is
/// written under the working directory itself. The content is a JSON object
/// recording the modified file path and timestamp so operators can correlate
/// what triggered the staleness.
pub(super) fn mark_blarify_stale_if_needed(state_root: &Path, tool_name: &str, tool_input: &Value) {
    let paths = extract_written_paths(tool_name, tool_input);
    let code_path = paths.iter().find(|p| is_code_file(p));
    let Some(path) = code_path else {
        return;
    };

    let project_root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::debug!("blarify staleness: no working directory ({}), skipped", e);
            return;
        }
    };
    let marker = blarify_marker_path(state_root, &project_root);
    let Some(marker_dir) = marker.parent() else {
        return;
    };

    if let Err(e) = create_private_dir_all(marker_dir) {
        tracing::warn!(
            "blarify staleness: failed to create {}: {}",
            marker_dir.display(),
            e
        );
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
        tracing::warn!(
            "blarify staleness: failed to write {}: {}",
            marker.display(),
            e
        );
    } else {
        tracing::debug!(
            "blarify staleness marker written (tool={}, path={})",
            tool_name,
            path
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // is_code_file
    // -----------------------------------------------------------------------

    #[test]
    fn code_file_rust() {
        assert!(is_code_file("src/main.rs"));
    }

    #[test]
    fn code_file_typescript() {
        assert!(is_code_file("app/component.tsx"));
    }

    #[test]
    fn code_file_case_insensitive() {
        assert!(is_code_file("Main.RS"));
    }

    #[test]
    fn non_code_file() {
        assert!(!is_code_file("README.md"));
        assert!(!is_code_file("data.json"));
        assert!(!is_code_file("config.yaml"));
    }

    // -----------------------------------------------------------------------
    // validate_tool_result
    // -----------------------------------------------------------------------

    #[test]
    fn validate_none_result() {
        assert!(validate_tool_result("Edit", None).is_none());
    }

    #[test]
    fn validate_error_field() {
        let result = json!({"error": "file not found"});
        let warning = validate_tool_result("Edit", Some(&result));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("file not found"));
    }

    #[test]
    fn validate_success_false() {
        let result = json!({"success": false, "message": "permission denied"});
        let warning = validate_tool_result("Write", Some(&result));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("permission denied"));
    }

    #[test]
    fn validate_success_true() {
        let result = json!({"success": true});
        assert!(validate_tool_result("Edit", Some(&result)).is_none());
    }

    #[test]
    fn validate_unknown_tool() {
        let result = json!({"error": "oops"});
        assert!(validate_tool_result("Bash", Some(&result)).is_none());
    }

    // -----------------------------------------------------------------------
    // extract_written_paths
    // -----------------------------------------------------------------------

    #[test]
    fn extract_write_path() {
        let input = json!({"path": "src/main.rs"});
        let paths = extract_written_paths("Write", &input);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn extract_create_path() {
        let input = json!({"path": "new_file.py"});
        let paths = extract_written_paths("create", &input);
        assert_eq!(paths, vec!["new_file.py"]);
    }

    #[test]
    fn extract_edit_file_path() {
        let input = json!({"file_path": "src/lib.rs"});
        let paths = extract_written_paths("Edit", &input);
        assert_eq!(paths, vec!["src/lib.rs"]);
    }

    #[test]
    fn extract_edit_fallback_path() {
        let input = json!({"path": "src/lib.rs"});
        let paths = extract_written_paths("edit", &input);
        assert_eq!(paths, vec!["src/lib.rs"]);
    }

    #[test]
    fn extract_multiedit_paths() {
        let input = json!({
            "edits": [
                {"file_path": "a.rs"},
                {"file_path": "b.rs"}
            ]
        });
        let paths = extract_written_paths("MultiEdit", &input);
        assert_eq!(paths, vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn blarify_marker_path_is_project_keyed_under_state_root() {
        let root = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let marker = blarify_marker_path(root.path(), project.path());
        let key_dir = marker.parent().unwrap();
        assert_eq!(marker.file_name().unwrap(), "blarify_stale");
        assert_eq!(
            key_dir.parent().unwrap(),
            root.path().join("runtime").join("projects")
        );
        let key = key_dir.file_name().unwrap().to_str().unwrap();
        assert_eq!(key.len(), 16);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
        assert_eq!(marker, blarify_marker_path(root.path(), project.path()));
    }

    #[test]
    fn blarify_marker_path_falls_back_to_uncanonicalised_path() {
        let root = Path::new("/state");
        let missing = Path::new("/definitely/not/a/real/project-1963");
        let a = blarify_marker_path(root, missing);
        let b = blarify_marker_path(root, Path::new("/definitely/not/a/real/project-1964"));
        assert_ne!(a, b);
        assert!(!a.to_string_lossy().contains("project-1963"));
    }

    #[test]
    fn extract_unknown_tool_empty() {
        let input = json!({"path": "file.rs"});
        assert!(extract_written_paths("Bash", &input).is_empty());
    }
}
