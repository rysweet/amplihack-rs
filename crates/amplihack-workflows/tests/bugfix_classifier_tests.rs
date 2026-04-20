//! TDD tests for classifier bug fix #269.
//!
//! Contract: OPS keywords must be multi-word phrases so that single
//! generic words like "cleanup" and "manage" don't steal development
//! tasks that incidentally mention those words.

use amplihack_workflows::classifier::{WorkflowClassifier, WorkflowType};
use std::collections::HashMap;

// ============================================================================
// Bug #269: Constructive-verb tasks must not be misclassified as Ops
// ============================================================================

#[test]
fn add_task_with_cleanup_in_filename_classifies_as_default() {
    // Regression: "Add an agentic disk-cleanup loop. Extend src/cmd_cleanup.rs"
    // was misclassified as OPS because "cleanup" matched as a substring.
    let c = WorkflowClassifier::default();
    let r = c.classify(
        "Add an agentic disk-cleanup loop. Extend src/cmd_cleanup.rs with a new function.",
    );
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "Task starting with 'Add' must be Default, not {:?}. Reason: {}",
        r.workflow,
        r.reason,
    );
}

#[test]
fn implement_task_mentioning_manage_classifies_as_default() {
    // "manage" as a standalone word was too broad for OPS.
    let c = WorkflowClassifier::default();
    let r = c.classify("Implement a new component to manage user sessions");
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "'Implement' must win over incidental 'manage'. Reason: {}",
        r.reason,
    );
}

#[test]
fn create_task_with_file_organization_classifies_as_default() {
    // "organize" was an OPS keyword that could steal development tasks.
    let c = WorkflowClassifier::default();
    let r = c.classify("Create a utility to organize test fixtures by category");
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "'Create' must win. Reason: {}",
        r.reason,
    );
}

#[test]
fn explicit_ops_phrase_classifies_as_ops() {
    // Multi-word OPS phrases must still work when no Default keyword is present.
    let c = WorkflowClassifier::default();
    let r = c.classify("disk cleanup of temporary log files");
    assert_eq!(
        r.workflow,
        WorkflowType::Ops,
        "'disk cleanup' is a valid OPS phrase. Reason: {}",
        r.reason,
    );
}

#[test]
fn manage_repos_classifies_as_ops() {
    let c = WorkflowClassifier::default();
    let r = c.classify("manage repos across the organization");
    assert_eq!(
        r.workflow,
        WorkflowType::Ops,
        "'manage repos' is a valid OPS phrase. Reason: {}",
        r.reason,
    );
}

#[test]
fn git_operations_classifies_as_ops() {
    let c = WorkflowClassifier::default();
    let r = c.classify("run git operations to clean up stale branches");
    assert_eq!(
        r.workflow,
        WorkflowType::Ops,
        "'git operations' is OPS. Reason: {}",
        r.reason,
    );
}

#[test]
fn delete_files_classifies_as_default_when_combined_with_dev_verb() {
    // "delete files" matches OPS, but "delete" also matches Default.
    // Default has higher priority, so Default wins.
    let c = WorkflowClassifier::default();
    let r = c.classify("delete files that are no longer needed and fix the config");
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "Default keywords take priority over OPS. Reason: {}",
        r.reason,
    );
}

#[test]
fn bare_cleanup_word_does_not_match_ops() {
    // "cleanup" alone (not "disk cleanup") should NOT match OPS.
    let c = WorkflowClassifier::default();
    let r = c.classify("cleanup the codebase");
    // Without any Default keyword, this should fall through to default with
    // low confidence (ambiguous).
    assert_ne!(
        r.workflow,
        WorkflowType::Ops,
        "bare 'cleanup' must not match OPS. Reason: {}",
        r.reason,
    );
}

#[test]
fn bare_manage_word_does_not_match_ops() {
    // "manage" alone should NOT match OPS anymore.
    let c = WorkflowClassifier::default();
    let r = c.classify("manage the configuration settings");
    assert_ne!(
        r.workflow,
        WorkflowType::Ops,
        "bare 'manage' must not match OPS. Reason: {}",
        r.reason,
    );
}

#[test]
fn build_task_with_delete_reference_classifies_as_default() {
    // "build" is Default; even if "delete files" appears, Default priority wins.
    let c = WorkflowClassifier::default();
    let r = c.classify("build a tool that can delete files safely");
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "'build' must win. Reason: {}",
        r.reason,
    );
}

#[test]
fn refactor_task_mentioning_repo_management_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("refactor the repo management module for better performance");
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "'refactor' must win over 'repo management'. Reason: {}",
        r.reason,
    );
}

// ============================================================================
// Confidence scoring
// ============================================================================

#[test]
fn single_keyword_match_gives_lower_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("fix the bug");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert!(
        r.confidence >= 0.7 && r.confidence < 0.9,
        "single keyword match should give ~0.7 confidence, got {}",
        r.confidence,
    );
}

#[test]
fn multiple_keyword_matches_give_higher_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("implement and create a new feature to update the system");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert!(
        r.confidence >= 0.9,
        "multiple keyword matches should give >=0.9 confidence, got {}",
        r.confidence,
    );
}

#[test]
fn no_keyword_match_gives_low_confidence_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("do something with the system");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert!(
        r.confidence < 0.7,
        "no keyword match should give <0.7 confidence, got {}",
        r.confidence,
    );
}

// ============================================================================
// Empty / edge case inputs
// ============================================================================

#[test]
fn empty_input_returns_default_with_zero_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert_eq!(r.confidence, 0.0);
    assert!(r.keywords.is_empty());
}

#[test]
fn whitespace_only_input_returns_default_with_zero_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("   \t\n  ");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert_eq!(r.confidence, 0.0);
}

// ============================================================================
// Custom keywords extension
// ============================================================================

#[test]
fn custom_keywords_add_to_existing_not_replace() {
    let mut custom = HashMap::new();
    custom.insert(WorkflowType::QAndA, vec!["tell me about".to_string()]);
    let c = WorkflowClassifier::new(Some(custom));
    // Original QAndA keyword still works
    let r1 = c.classify("what is the purpose of this module");
    assert_eq!(r1.workflow, WorkflowType::QAndA);
    // Custom keyword also works
    let r2 = c.classify("tell me about the architecture");
    assert_eq!(r2.workflow, WorkflowType::QAndA);
}

// ============================================================================
// Priority ordering verification
// ============================================================================

#[test]
fn default_takes_priority_over_investigation() {
    let c = WorkflowClassifier::default();
    // "fix" (Default) + "investigate" (Investigation)
    let r = c.classify("fix the issue after you investigate the root cause");
    assert_eq!(
        r.workflow,
        WorkflowType::Default,
        "Default must take priority over Investigation. Reason: {}",
        r.reason,
    );
}

#[test]
fn investigation_takes_priority_over_ops() {
    let c = WorkflowClassifier::default();
    // "investigate" (Investigation) + "disk cleanup" (Ops)
    let r = c.classify("investigate why disk cleanup is slow");
    assert_eq!(
        r.workflow,
        WorkflowType::Investigation,
        "Investigation must take priority over Ops. Reason: {}",
        r.reason,
    );
}

#[test]
fn investigation_takes_priority_over_qa() {
    let c = WorkflowClassifier::default();
    // "investigate" (Investigation) + "what is" (Q&A)
    let r = c.classify("investigate what is causing the failure");
    // "investigate" → Investigation, "what is" → Q&A
    // Investigation has higher priority
    assert_eq!(
        r.workflow,
        WorkflowType::Investigation,
        "Investigation must take priority over Q&A. Reason: {}",
        r.reason,
    );
}
