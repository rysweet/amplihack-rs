//! Bugfix integration tests for workflow classifier (#269).
//!
//! Validates that the constructive-verb override prevents misclassification
//! of multi-requirement "Add" tasks as Ops.

use amplihack_workflows::classifier::{WorkflowClassifier, WorkflowType};

// ── Misclassification regression tests (issue #269) ──

#[test]
fn add_manage_users_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Add a feature to manage users in the admin panel");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn create_management_dashboard_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Create a management dashboard for monitoring");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn implement_cleanup_policy_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Implement a cleanup policy for expired sessions");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn build_cleanup_tool_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Build a cleanup tool for database records");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn design_organize_component_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Design an organize files component for the UI");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn extend_cleanup_feature_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Extend the cleanup feature to handle batch deletes");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn add_cleanup_admin_panel_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Add a cleanup feature to the admin panel");
    assert_eq!(r.workflow, WorkflowType::Default);
}

// ── Legitimate OPS tasks stay as OPS ──

#[test]
fn manage_infrastructure_classifies_as_ops() {
    let c = WorkflowClassifier::default();
    let r = c.classify("manage infrastructure for the production cluster");
    assert_eq!(r.workflow, WorkflowType::Ops);
}

#[test]
fn manage_deployment_classifies_as_ops() {
    let c = WorkflowClassifier::default();
    let r = c.classify("manage deployment of the staging environment");
    assert_eq!(r.workflow, WorkflowType::Ops);
}

#[test]
fn disk_cleanup_classifies_as_ops() {
    let c = WorkflowClassifier::default();
    let r = c.classify("disk cleanup of /var/log files");
    assert_eq!(r.workflow, WorkflowType::Ops);
}

// ── Priority ordering ──

#[test]
fn default_takes_priority_over_investigation() {
    let c = WorkflowClassifier::default();
    let r = c.classify("implement and investigate the logging module");
    assert_eq!(r.workflow, WorkflowType::Default);
}

#[test]
fn default_takes_priority_over_ops() {
    let c = WorkflowClassifier::default();
    let r = c.classify("delete files and fix the build");
    assert_eq!(r.workflow, WorkflowType::Default);
}

// ── Confidence scoring ──

#[test]
fn constructive_verb_override_has_reasonable_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("Add a cleanup feature to the admin panel");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert!(
        r.confidence >= 0.5,
        "overridden classification should have confidence >= 0.5, got {}",
        r.confidence
    );
}

#[test]
fn single_keyword_match_has_standard_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("investigate why the tests fail");
    assert_eq!(r.workflow, WorkflowType::Investigation);
    assert!(
        r.confidence >= 0.7,
        "single keyword match should have confidence >= 0.7"
    );
}

#[test]
fn ambiguous_request_has_low_confidence() {
    let c = WorkflowClassifier::default();
    let r = c.classify("do something with the system");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert!(r.confidence < 0.7);
}

// ── Edge cases ──

#[test]
fn empty_request_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert_eq!(r.confidence, 0.0);
}

#[test]
fn whitespace_only_classifies_as_default() {
    let c = WorkflowClassifier::default();
    let r = c.classify("   \n\t  ");
    assert_eq!(r.workflow, WorkflowType::Default);
    assert_eq!(r.confidence, 0.0);
}
