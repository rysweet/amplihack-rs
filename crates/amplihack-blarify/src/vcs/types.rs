//! DTOs for version control operations.
//!
//! Mirrors the Python `repositories/version_control/dtos/` types.

use serde::{Deserialize, Serialize};

/// Line range for blame attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameLineRange {
    pub start: i64,
    pub end: i64,
}

/// Pull request information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestInfo {
    pub number: i64,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default = "default_pr_state")]
    pub state: String,
    #[serde(default)]
    pub body_text: Option<String>,
}

fn default_pr_state() -> String {
    "MERGED".into()
}

/// Blame information for a commit affecting specific line ranges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlameCommit {
    pub sha: String,
    pub message: String,
    pub author: String,
    #[serde(default)]
    pub author_email: Option<String>,
    #[serde(default)]
    pub author_login: Option<String>,
    pub timestamp: String,
    pub url: String,
    #[serde(default)]
    pub additions: Option<i64>,
    #[serde(default)]
    pub deletions: Option<i64>,
    pub line_ranges: Vec<BlameLineRange>,
    #[serde(default)]
    pub pr_info: Option<PullRequestInfo>,
}

/// File change in a commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub filename: String,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    #[serde(default)]
    pub patch: Option<String>,
    #[serde(default)]
    pub previous_filename: Option<String>,
}

/// Repository information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub name: String,
    pub owner: String,
    pub url: String,
    pub default_branch: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Commit information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    #[serde(default)]
    pub author_email: Option<String>,
    pub timestamp: String,
    pub url: String,
    #[serde(default)]
    pub pr_number: Option<i64>,
}

/// Parsed patch header with line ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchLineRange {
    pub start_line: i64,
    pub line_count: i64,
}

/// Parsed patch header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchHeader {
    pub deleted: Option<PatchLineRange>,
    pub added: Option<PatchLineRange>,
}

/// A single change range extracted from a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRange {
    pub change_type: String,
    pub line_start: i64,
    pub line_end: i64,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blame_line_range_roundtrip() {
        let range = BlameLineRange { start: 10, end: 20 };
        let json = serde_json::to_string(&range).unwrap();
        let deser: BlameLineRange = serde_json::from_str(&json).unwrap();
        assert_eq!(range, deser);
    }

    #[test]
    fn pull_request_info_defaults() {
        let json = r#"{"number":42,"title":"Fix bug","url":"https://github.com/r/p/pull/42"}"#;
        let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.state, "MERGED");
        assert!(pr.author.is_none());
    }

    #[test]
    fn blame_commit_full() {
        let commit = BlameCommit {
            sha: "abc123".into(),
            message: "Fix issue".into(),
            author: "dev".into(),
            author_email: Some("dev@example.com".into()),
            author_login: None,
            timestamp: "2024-01-01T00:00:00Z".into(),
            url: "https://github.com/r/p/commit/abc123".into(),
            additions: Some(10),
            deletions: Some(5),
            line_ranges: vec![BlameLineRange { start: 1, end: 15 }],
            pr_info: None,
        };
        assert_eq!(commit.sha, "abc123");
        assert_eq!(commit.line_ranges.len(), 1);
    }

    #[test]
    fn file_change_serialization() {
        let change = FileChange {
            filename: "src/main.rs".into(),
            status: "modified".into(),
            additions: 5,
            deletions: 2,
            patch: Some("@@ -1,3 +1,5 @@".into()),
            previous_filename: None,
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("modified"));
    }

    #[test]
    fn patch_header_equality() {
        let h1 = PatchHeader {
            deleted: Some(PatchLineRange {
                start_line: 45,
                line_count: 7,
            }),
            added: Some(PatchLineRange {
                start_line: 45,
                line_count: 15,
            }),
        };
        let h2 = h1.clone();
        assert_eq!(h1, h2);
    }
}
