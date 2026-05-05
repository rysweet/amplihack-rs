//! R1: Issue dedup module — fingerprint-based GitHub issue deduplication.
//!
//! Provides trait-abstracted GitHub issue management to prevent duplicate issue
//! filing by the orchestrator auto-filer. Uses SHA-256 fingerprints keyed on
//! `(error_class, day)` to decide whether to create a new issue, append a
//! comment to an existing one, or create a daily rollup.

use sha2::{Digest, Sha256};
use std::fmt;

// ---------------------------------------------------------------------------
// Fingerprint
// ---------------------------------------------------------------------------

/// A 12-char hex fingerprint derived from error class fields.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub String);

impl Fingerprint {
    /// Build fingerprint from error class components.
    /// Matches the `make_signature_id` pattern in `amplihack-recovery::stage2`.
    pub fn from_error_class(error_type: &str, headline: &str, location: &str) -> Self {
        let canonical = format!("{error_type}|{headline}|{location}");
        let hash = Sha256::digest(canonical.as_bytes());
        let hex: String = hash[..6].iter().map(|b| format!("{b:02x}")).collect();
        Self(hex)
    }

    /// Build the full dedup key combining fingerprint + date.
    pub fn dedup_key(&self, date_ymd: &str) -> String {
        format!("{}:{}", self.0, date_ymd)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Issue search result
// ---------------------------------------------------------------------------

/// Represents a found GitHub issue matching a fingerprint search.
#[derive(Clone, Debug)]
pub struct FoundIssue {
    pub number: u64,
    pub title: String,
    pub created_date: String, // YYYY-MM-DD
    pub is_open: bool,
}

// ---------------------------------------------------------------------------
// Dedup decision
// ---------------------------------------------------------------------------

/// The action the dedup logic decided to take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DedupAction {
    /// No matching issue exists today — create a new one.
    CreateNew,
    /// An open issue with the same fingerprint was created today — append a comment.
    AppendComment { issue_number: u64 },
    /// Open issues exist from prior days — create a new daily issue that cross-references them.
    CreateDailyRollup { prior_issue_numbers: Vec<u64> },
}

// ---------------------------------------------------------------------------
// IssueClient trait
// ---------------------------------------------------------------------------

/// Abstraction over GitHub issue operations (real impl shells out to `gh`).
pub trait IssueClient {
    /// Search for open issues containing the fingerprint marker.
    fn search_issues_by_fingerprint(
        &self,
        repo: &str,
        fingerprint: &Fingerprint,
    ) -> Result<Vec<FoundIssue>, String>;

    /// Create a new issue. Returns issue number.
    fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> Result<u64, String>;

    /// Append a comment to an existing issue.
    fn add_comment(&self, repo: &str, issue_number: u64, body: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Core dedup logic
// ---------------------------------------------------------------------------

/// Determine what action to take given the current date and search results.
pub fn decide_action(
    matching_issues: &[FoundIssue],
    today: &str, // YYYY-MM-DD
) -> DedupAction {
    // Filter to open issues only
    let open_issues: Vec<&FoundIssue> = matching_issues.iter().filter(|i| i.is_open).collect();

    if open_issues.is_empty() {
        return DedupAction::CreateNew;
    }

    // Check if any open issue was created today
    let todays_issues: Vec<&FoundIssue> = open_issues
        .iter()
        .filter(|i| i.created_date == today)
        .copied()
        .collect();

    if let Some(issue) = todays_issues.first() {
        return DedupAction::AppendComment {
            issue_number: issue.number,
        };
    }

    // Open issues exist but none from today — daily rollup
    let prior_numbers: Vec<u64> = open_issues.iter().map(|i| i.number).collect();
    DedupAction::CreateDailyRollup {
        prior_issue_numbers: prior_numbers,
    }
}

/// Execute the full dedup workflow: search, decide, act.
pub fn file_or_dedup(
    client: &dyn IssueClient,
    repo: &str,
    fingerprint: &Fingerprint,
    today: &str,
    error_title: &str,
    error_body: &str,
) -> Result<DedupAction, String> {
    let matches = client.search_issues_by_fingerprint(repo, fingerprint)?;
    let action = decide_action(&matches, today);

    match &action {
        DedupAction::CreateNew => {
            let body_with_marker =
                format!("{error_body}\n\n<!-- amplihack-fingerprint:{fingerprint} -->");
            client.create_issue(repo, error_title, &body_with_marker, &["auto-filed"])?;
        }
        DedupAction::AppendComment { issue_number } => {
            let comment = format!("**Duplicate occurrence** ({today})\n\n{error_body}");
            client.add_comment(repo, *issue_number, &comment)?;
        }
        DedupAction::CreateDailyRollup {
            prior_issue_numbers,
        } => {
            let refs: Vec<String> = prior_issue_numbers
                .iter()
                .map(|n| format!("#{n}"))
                .collect();
            let rollup_body = format!(
                "**Daily rollup** for {today}\n\nPrior issues: {}\n\n{error_body}\n\n<!-- amplihack-fingerprint:{} -->",
                refs.join(", "),
                fingerprint
            );
            client.create_issue(
                repo,
                &format!("[Rollup {today}] {error_title}"),
                &rollup_body,
                &["auto-filed"],
            )?;
        }
    }

    Ok(action)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Mock IssueClient
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockIssueClient {
        /// Pre-loaded search results keyed by fingerprint string.
        search_results: HashMap<String, Vec<FoundIssue>>,
        /// Track created issues: (title, body, labels)
        created_issues: RefCell<Vec<(String, String, Vec<String>)>>,
        /// Track appended comments: (issue_number, body)
        appended_comments: RefCell<Vec<(u64, String)>>,
        /// Next issue number to return from create.
        next_issue_number: RefCell<u64>,
    }

    impl MockIssueClient {
        fn with_search_results(mut self, fp: &Fingerprint, issues: Vec<FoundIssue>) -> Self {
            self.search_results.insert(fp.0.clone(), issues);
            self
        }
    }

    impl IssueClient for MockIssueClient {
        fn search_issues_by_fingerprint(
            &self,
            _repo: &str,
            fingerprint: &Fingerprint,
        ) -> Result<Vec<FoundIssue>, String> {
            Ok(self
                .search_results
                .get(&fingerprint.0)
                .cloned()
                .unwrap_or_default())
        }

        fn create_issue(
            &self,
            _repo: &str,
            title: &str,
            body: &str,
            labels: &[&str],
        ) -> Result<u64, String> {
            let num = {
                let mut n = self.next_issue_number.borrow_mut();
                *n += 1;
                *n
            };
            self.created_issues.borrow_mut().push((
                title.to_string(),
                body.to_string(),
                labels.iter().map(|s| s.to_string()).collect(),
            ));
            Ok(num)
        }

        fn add_comment(&self, _repo: &str, issue_number: u64, body: &str) -> Result<(), String> {
            self.appended_comments
                .borrow_mut()
                .push((issue_number, body.to_string()));
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Fingerprint tests
    // -----------------------------------------------------------------------

    #[test]
    fn fingerprint_is_12_hex_chars() {
        let fp = Fingerprint::from_error_class("ImportError", "no module foo", "src/main.py");
        assert_eq!(fp.0.len(), 12, "fingerprint must be 12 hex chars");
        assert!(fp.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_deterministic() {
        let fp1 = Fingerprint::from_error_class("TypeError", "expected int", "lib.rs:42");
        let fp2 = Fingerprint::from_error_class("TypeError", "expected int", "lib.rs:42");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn fingerprint_differs_for_different_errors() {
        let fp1 = Fingerprint::from_error_class("TypeError", "expected int", "lib.rs:42");
        let fp2 = Fingerprint::from_error_class("ValueError", "expected int", "lib.rs:42");
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn dedup_key_format() {
        let fp = Fingerprint::from_error_class("Err", "msg", "loc");
        let key = fp.dedup_key("2026-04-23");
        assert!(key.contains("2026-04-23"));
        assert!(key.starts_with(&fp.0));
    }

    // -----------------------------------------------------------------------
    // decide_action tests — the 3 code paths
    // -----------------------------------------------------------------------

    #[test]
    fn decide_create_new_when_no_matches() {
        let action = decide_action(&[], "2026-04-23");
        assert_eq!(action, DedupAction::CreateNew);
    }

    #[test]
    fn decide_create_new_when_only_closed_issues() {
        let issues = vec![FoundIssue {
            number: 10,
            title: "old".into(),
            created_date: "2026-04-23".into(),
            is_open: false,
        }];
        let action = decide_action(&issues, "2026-04-23");
        assert_eq!(action, DedupAction::CreateNew);
    }

    #[test]
    fn decide_append_comment_when_open_issue_exists_today() {
        let issues = vec![FoundIssue {
            number: 42,
            title: "existing".into(),
            created_date: "2026-04-23".into(),
            is_open: true,
        }];
        let action = decide_action(&issues, "2026-04-23");
        assert_eq!(action, DedupAction::AppendComment { issue_number: 42 });
    }

    #[test]
    fn decide_daily_rollup_when_open_issues_from_prior_days() {
        let issues = vec![
            FoundIssue {
                number: 10,
                title: "day1".into(),
                created_date: "2026-04-21".into(),
                is_open: true,
            },
            FoundIssue {
                number: 20,
                title: "day2".into(),
                created_date: "2026-04-22".into(),
                is_open: true,
            },
        ];
        let action = decide_action(&issues, "2026-04-23");
        assert_eq!(
            action,
            DedupAction::CreateDailyRollup {
                prior_issue_numbers: vec![10, 20],
            }
        );
    }

    #[test]
    fn decide_prefers_today_append_over_rollup() {
        // Mix of today + prior day open issues → should append to today's
        let issues = vec![
            FoundIssue {
                number: 10,
                title: "yesterday".into(),
                created_date: "2026-04-22".into(),
                is_open: true,
            },
            FoundIssue {
                number: 42,
                title: "today".into(),
                created_date: "2026-04-23".into(),
                is_open: true,
            },
        ];
        let action = decide_action(&issues, "2026-04-23");
        assert_eq!(action, DedupAction::AppendComment { issue_number: 42 });
    }

    // -----------------------------------------------------------------------
    // file_or_dedup integration tests with mock
    // -----------------------------------------------------------------------

    #[test]
    fn file_or_dedup_creates_issue_with_fingerprint_marker() {
        let fp = Fingerprint::from_error_class("TestError", "boom", "test.rs:1");
        let client = MockIssueClient::default();

        let action = file_or_dedup(
            &client,
            "rysweet/amplihack-rs",
            &fp,
            "2026-04-23",
            "TestError: boom",
            "Stack trace here",
        )
        .unwrap();

        assert_eq!(action, DedupAction::CreateNew);
        let created = client.created_issues.borrow();
        assert_eq!(created.len(), 1);
        assert!(created[0].1.contains("amplihack-fingerprint:"));
        assert!(created[0].2.contains(&"auto-filed".to_string()));
    }

    #[test]
    fn file_or_dedup_appends_comment_on_today_match() {
        let fp = Fingerprint::from_error_class("TestError", "boom", "test.rs:1");
        let client = MockIssueClient::default().with_search_results(
            &fp,
            vec![FoundIssue {
                number: 99,
                title: "existing".into(),
                created_date: "2026-04-23".into(),
                is_open: true,
            }],
        );

        let action = file_or_dedup(
            &client,
            "rysweet/amplihack-rs",
            &fp,
            "2026-04-23",
            "TestError: boom",
            "Another occurrence",
        )
        .unwrap();

        assert_eq!(action, DedupAction::AppendComment { issue_number: 99 });
        let comments = client.appended_comments.borrow();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].0, 99);
        assert!(comments[0].1.contains("Duplicate occurrence"));
    }

    #[test]
    fn file_or_dedup_creates_rollup_with_cross_references() {
        let fp = Fingerprint::from_error_class("TestError", "boom", "test.rs:1");
        let client = MockIssueClient::default().with_search_results(
            &fp,
            vec![FoundIssue {
                number: 50,
                title: "old".into(),
                created_date: "2026-04-20".into(),
                is_open: true,
            }],
        );

        let action = file_or_dedup(
            &client,
            "rysweet/amplihack-rs",
            &fp,
            "2026-04-23",
            "TestError: boom",
            "Details",
        )
        .unwrap();

        assert_eq!(
            action,
            DedupAction::CreateDailyRollup {
                prior_issue_numbers: vec![50],
            }
        );
        let created = client.created_issues.borrow();
        assert_eq!(created.len(), 1);
        assert!(created[0].0.contains("Rollup"));
        assert!(created[0].1.contains("#50"));
    }

    #[test]
    fn file_or_dedup_propagates_search_error() {
        struct FailingClient;
        impl IssueClient for FailingClient {
            fn search_issues_by_fingerprint(
                &self,
                _repo: &str,
                _fingerprint: &Fingerprint,
            ) -> Result<Vec<FoundIssue>, String> {
                Err("gh: not found".into())
            }
            fn create_issue(
                &self,
                _repo: &str,
                _title: &str,
                _body: &str,
                _labels: &[&str],
            ) -> Result<u64, String> {
                unreachable!()
            }
            fn add_comment(
                &self,
                _repo: &str,
                _issue_number: u64,
                _body: &str,
            ) -> Result<(), String> {
                unreachable!()
            }
        }

        let fp = Fingerprint::from_error_class("E", "h", "l");
        let result = file_or_dedup(&FailingClient, "repo", &fp, "2026-04-23", "t", "b");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("gh: not found"));
    }
}
