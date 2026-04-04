//! Abstract version controller trait.
//!
//! Mirrors the Python `repositories/version_control/abstract_version_controller.py`.

use std::collections::HashMap;

use anyhow::Result;

use super::types::{
    BlameCommit, ChangeRange, CommitInfo, FileChange, PatchHeader, PatchLineRange, RepositoryInfo,
};

/// Abstract interface for version control system integrations.
///
/// Implementations cover GitHub, GitLab, Bitbucket, etc.
pub trait VersionController: Send + Sync {
    /// Fetch merged pull requests from the VCS.
    fn fetch_pull_requests(
        &self,
        limit: usize,
        since_date: Option<&str>,
    ) -> Result<Vec<serde_json::Value>>;

    /// Fetch commits, optionally filtered by PR, branch, or date.
    fn fetch_commits(
        &self,
        pr_number: Option<i64>,
        branch: Option<&str>,
        since_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommitInfo>>;

    /// Fetch file changes for a specific commit.
    fn fetch_commit_changes(&self, commit_sha: &str) -> Result<Vec<FileChange>>;

    /// Fetch file contents at a specific commit.
    fn fetch_file_at_commit(&self, file_path: &str, commit_sha: &str) -> Result<Option<String>>;

    /// Get repository information.
    fn get_repository_info(&self) -> Result<RepositoryInfo>;

    /// Test the connection to the VCS.
    fn test_connection(&self) -> Result<bool>;

    /// Get blame commits for a specific line range.
    fn blame_commits_for_range(
        &self,
        file_path: &str,
        start_line: i64,
        end_line: i64,
    ) -> Result<Vec<BlameCommit>>;

    /// Get blame commits for multiple code nodes efficiently.
    fn blame_commits_for_nodes(
        &self,
        nodes: &[serde_json::Value],
    ) -> Result<HashMap<String, Vec<BlameCommit>>>;
}

/// Parse a patch header to extract line range information.
///
/// Input format: `@@ -start,count +start,count @@`
pub fn parse_patch_header(patch_header: &str) -> PatchHeader {
    let re = regex::Regex::new(r"@@ -(\d+),?(\d*) \+(\d+),?(\d*) @@").unwrap();

    match re.captures(patch_header) {
        Some(caps) => {
            let deleted_start: i64 = caps[1].parse().unwrap_or(0);
            let deleted_count: i64 = caps
                .get(2)
                .and_then(|m| {
                    let s = m.as_str();
                    if s.is_empty() { None } else { s.parse().ok() }
                })
                .unwrap_or(1);
            let added_start: i64 = caps[3].parse().unwrap_or(0);
            let added_count: i64 = caps
                .get(4)
                .and_then(|m| {
                    let s = m.as_str();
                    if s.is_empty() { None } else { s.parse().ok() }
                })
                .unwrap_or(1);

            PatchHeader {
                deleted: Some(PatchLineRange {
                    start_line: deleted_start,
                    line_count: deleted_count,
                }),
                added: Some(PatchLineRange {
                    start_line: added_start,
                    line_count: added_count,
                }),
            }
        }
        None => PatchHeader {
            deleted: None,
            added: None,
        },
    }
}

/// Extract specific line and character ranges for each change in a patch.
///
/// Groups consecutive lines of the same type (addition/deletion) into single ranges.
pub fn extract_change_ranges(patch: &str) -> Vec<ChangeRange> {
    let mut changes = Vec::new();
    let mut current_old_line: i64 = 0;
    let mut current_new_line: i64 = 0;
    let mut current_change: Option<ChangeRange> = None;

    for line in patch.lines() {
        if line.starts_with("@@") {
            if let Some(change) = current_change.take() {
                changes.push(change);
            }
            let header = parse_patch_header(line);
            current_old_line = header.deleted.as_ref().map_or(0, |d| d.start_line);
            current_new_line = header.added.as_ref().map_or(0, |a| a.start_line);
        } else if let Some(stripped) = line.strip_prefix('-') {
            if !line.starts_with("---") {
                let content_text = stripped;
                if let Some(ref mut change) = current_change {
                    if change.change_type == "deletion" && change.line_end == current_old_line - 1 {
                        change.line_end = current_old_line;
                        change.content.push('\n');
                        change.content.push_str(content_text);
                    } else {
                        changes.push(current_change.take().unwrap());
                        current_change = Some(ChangeRange {
                            change_type: "deletion".into(),
                            line_start: current_old_line,
                            line_end: current_old_line,
                            content: content_text.to_string(),
                        });
                    }
                } else {
                    current_change = Some(ChangeRange {
                        change_type: "deletion".into(),
                        line_start: current_old_line,
                        line_end: current_old_line,
                        content: content_text.to_string(),
                    });
                }
                current_old_line += 1;
            }
        } else if let Some(stripped) = line.strip_prefix('+') {
            if !line.starts_with("+++") {
                let content_text = stripped;
                if let Some(ref mut change) = current_change {
                    if change.change_type == "addition" && change.line_end == current_new_line - 1 {
                        change.line_end = current_new_line;
                        change.content.push('\n');
                        change.content.push_str(content_text);
                    } else {
                        changes.push(current_change.take().unwrap());
                        current_change = Some(ChangeRange {
                            change_type: "addition".into(),
                            line_start: current_new_line,
                            line_end: current_new_line,
                            content: content_text.to_string(),
                        });
                    }
                } else {
                    current_change = Some(ChangeRange {
                        change_type: "addition".into(),
                        line_start: current_new_line,
                        line_end: current_new_line,
                        content: content_text.to_string(),
                    });
                }
                current_new_line += 1;
            }
        } else if !line.is_empty() && !line.starts_with('\\') {
            if let Some(change) = current_change.take() {
                changes.push(change);
            }
            current_old_line += 1;
            current_new_line += 1;
        }
    }

    if let Some(change) = current_change {
        changes.push(change);
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_patch_header() {
        let header = parse_patch_header("@@ -45,7 +45,15 @@");
        let deleted = header.deleted.unwrap();
        let added = header.added.unwrap();
        assert_eq!(deleted.start_line, 45);
        assert_eq!(deleted.line_count, 7);
        assert_eq!(added.start_line, 45);
        assert_eq!(added.line_count, 15);
    }

    #[test]
    fn parse_patch_header_without_count() {
        let header = parse_patch_header("@@ -10 +10 @@");
        let deleted = header.deleted.unwrap();
        assert_eq!(deleted.line_count, 1);
    }

    #[test]
    fn parse_invalid_header() {
        let header = parse_patch_header("not a patch header");
        assert!(header.deleted.is_none());
        assert!(header.added.is_none());
    }

    #[test]
    fn extract_simple_changes() {
        let patch = "@@ -1,3 +1,4 @@\n context\n-old line\n+new line\n+added line\n context";
        let changes = extract_change_ranges(patch);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_type, "deletion");
        assert_eq!(changes[1].change_type, "addition");
        assert_eq!(changes[1].line_start, 2);
    }

    #[test]
    fn extract_empty_patch() {
        let changes = extract_change_ranges("");
        assert!(changes.is_empty());
    }
}
