use super::*;
use super::*;

#[test]
fn todo_state_new_valid() {
    let ts = TodoState::new(5, 3, 1, 1).unwrap();
    assert_eq!(ts.total, 5);
    assert_eq!(ts.completed, 3);
}

#[test]
fn todo_state_new_invalid() {
    let err = TodoState::new(5, 3, 1, 0);
    assert!(err.is_err());
}

#[test]
fn todo_state_empty() {
    let ts = TodoState::empty();
    assert_eq!(ts.total, 0);
}

#[test]
fn github_state_empty() {
    let gs = GitHubState::empty();
    assert!(gs.pr_number.is_none());
}

struct MockExtractor(Vec<serde_json::Value>);
impl TodoExtractor for MockExtractor {
    fn extract_todos(&self) -> Vec<serde_json::Value> {
        self.0.clone()
    }
}

#[test]
fn extract_todo_state_empty() {
    let ext = MockExtractor(vec![]);
    let state = WorkSummaryGenerator::extract_todo_state(&ext);
    assert_eq!(state, TodoState::empty());
}

#[test]
fn extract_todo_state_counts() {
    let ext = MockExtractor(vec![
        serde_json::json!({"status": "completed"}),
        serde_json::json!({"status": "in_progress"}),
        serde_json::json!({"status": "pending"}),
        serde_json::json!({"status": "pending"}),
    ]);
    let state = WorkSummaryGenerator::extract_todo_state(&ext);
    assert_eq!(state.total, 4);
    assert_eq!(state.completed, 1);
    assert_eq!(state.in_progress, 1);
    assert_eq!(state.pending, 2);
}

#[test]
fn format_for_prompt_no_tasks() {
    let summary = WorkSummary {
        todo_state: TodoState::empty(),
        git_state: GitState {
            current_branch: None,
            has_uncommitted_changes: false,
            commits_ahead: None,
        },
        github_state: GitHubState::empty(),
    };
    let text = WorkSummaryGenerator::format_for_prompt(&summary);
    assert!(text.contains("No TodoWrite entries"));
    assert!(text.contains("Not in repository"));
    assert!(text.contains("PR: not created"));
}

#[test]
fn format_for_prompt_with_data() {
    let summary = WorkSummary {
        todo_state: TodoState {
            total: 3,
            completed: 2,
            in_progress: 1,
            pending: 0,
        },
        git_state: GitState {
            current_branch: Some("feat/test".into()),
            has_uncommitted_changes: true,
            commits_ahead: Some(5),
        },
        github_state: GitHubState {
            pr_number: Some(42),
            pr_state: Some("OPEN".into()),
            ci_status: Some("SUCCESS".into()),
            pr_mergeable: Some(true),
        },
    };
    let text = WorkSummaryGenerator::format_for_prompt(&summary);
    assert!(text.contains("2/3 tasks completed"));
    assert!(text.contains("feat/test"));
    assert!(text.contains("Commits ahead: 5"));
    assert!(text.contains("Uncommitted changes: Yes"));
    assert!(text.contains("#42"));
    assert!(text.contains("passing"));
    assert!(text.contains("Mergeable: yes"));
}

#[test]
fn format_for_prompt_no_pr_mergeable_conflicts() {
    let summary = WorkSummary {
        todo_state: TodoState::empty(),
        git_state: GitState {
            current_branch: Some("main".into()),
            has_uncommitted_changes: false,
            commits_ahead: None,
        },
        github_state: GitHubState {
            pr_number: Some(10),
            pr_state: Some("OPEN".into()),
            ci_status: None,
            pr_mergeable: Some(false),
        },
    };
    let text = WorkSummaryGenerator::format_for_prompt(&summary);
    assert!(text.contains("no (conflicts)"));
}

#[test]
fn generator_caches_result() {
    let ext = MockExtractor(vec![]);
    let mut generator = WorkSummaryGenerator::new();
    let s1 = generator.generate(&ext);
    let s2 = generator.generate(&ext);
    // Cached — should be the same object data
    assert_eq!(s1.todo_state, s2.todo_state);
}
