//! High-level reflection orchestrator (port of `reflection.py`).
//!
//! Composes the analyzer, semaphore, state machine, and security
//! sanitization to drive a single reflection pass.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error_analysis::{ContextualErrorAnalyzer, ErrorAnalysis};
use crate::lightweight_analyzer::{
    AnalysisResult as LightweightResult, LightweightAnalyzer, Message,
};
use crate::semaphore::ReflectionLock;
use crate::state_machine::{ReflectionState, ReflectionStateData, ReflectionStateMachine};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionReport {
    pub session_id: String,
    pub state: ReflectionState,
    pub lightweight: LightweightResult,
    pub error_analysis: Option<ErrorAnalysis>,
    pub locked: bool,
}

/// Orchestrates a single reflection pass for a session.
pub struct ReflectionOrchestrator<'a> {
    pub session_id: String,
    pub runtime_dir: &'a Path,
}

impl<'a> ReflectionOrchestrator<'a> {
    pub fn new(session_id: impl Into<String>, runtime_dir: &'a Path) -> Self {
        Self {
            session_id: session_id.into(),
            runtime_dir,
        }
    }

    /// Run the analyzer pipeline. Acquires the loop-prevention lock and
    /// records analysis state to disk on success.
    pub fn run(
        &self,
        messages: &[Message],
        tool_logs: &[String],
        error_content: Option<&str>,
    ) -> anyhow::Result<ReflectionReport> {
        let lock = ReflectionLock::new(self.runtime_dir)?;
        let acquired = lock.acquire(&self.session_id, "analysis")?;
        if !acquired {
            return Ok(ReflectionReport {
                session_id: self.session_id.clone(),
                state: ReflectionState::Idle,
                lightweight: LightweightResult {
                    patterns: vec![],
                    summary: "Skipped — another reflection in progress".to_string(),
                    elapsed_seconds: 0.0,
                },
                error_analysis: None,
                locked: true,
            });
        }

        let sm = ReflectionStateMachine::new(&self.session_id, self.runtime_dir)?;
        let mut data = ReflectionStateData::new(&self.session_id);
        data.state = ReflectionState::Analyzing;
        sm.write_state(&data)?;

        let lightweight =
            LightweightAnalyzer::new().analyze_recent_responses(messages, tool_logs)?;
        let err = match error_content {
            Some(c) if !c.is_empty() => {
                Some(ContextualErrorAnalyzer::new().analyze_error_context(c, "")?)
            }
            _ => None,
        };

        data.analysis = Some(serde_json::json!({
            "lightweight": &lightweight,
            "error": &err,
        }));
        data.state = if lightweight.patterns.is_empty() && err.is_none() {
            ReflectionState::Completed
        } else {
            ReflectionState::AwaitingApproval
        };
        sm.write_state(&data)?;
        let final_state = data.state;
        lock.release()?;
        Ok(ReflectionReport {
            session_id: self.session_id.clone(),
            state: final_state,
            lightweight,
            error_analysis: err,
            locked: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_analysis::ErrorCategory;
    use crate::lightweight_analyzer::{PatternKind, Role};

    fn message(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
        }
    }

    fn assistant_msg(content: &str) -> Message {
        message(Role::Assistant, content)
    }

    fn user_msg(content: &str) -> Message {
        message(Role::User, content)
    }

    #[test]
    fn new_stores_session_id_and_runtime_dir() {
        let tmp = tempfile::tempdir().unwrap();

        let orchestrator = ReflectionOrchestrator::new("session-42", tmp.path());

        assert_eq!(orchestrator.session_id, "session-42");
        assert_eq!(orchestrator.runtime_dir, tmp.path());
    }

    #[test]
    fn run_with_clean_recent_messages_completes_without_error_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("clean-session", tmp.path());
        let messages = vec![
            user_msg("Summarize the current status."),
            assistant_msg("All requested work is complete."),
        ];

        let report = orchestrator.run(&messages, &[], None).unwrap();

        assert_eq!(report.session_id, "clean-session");
        assert_eq!(report.state, ReflectionState::Completed);
        assert!(!report.locked);
        assert!(report.lightweight.patterns.is_empty());
        assert!(report.error_analysis.is_none());
    }

    #[test]
    fn run_with_detected_lightweight_patterns_awaits_approval() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("pattern-session", tmp.path());
        let messages = vec![
            user_msg("Run the build."),
            assistant_msg("The build failed with an error."),
        ];

        let report = orchestrator.run(&messages, &[], None).unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert!(!report.locked);
        assert!(
            report
                .lightweight
                .patterns
                .iter()
                .any(|pattern| pattern.kind == PatternKind::Error)
        );
    }

    #[test]
    fn run_returns_locked_report_when_reflection_lock_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = ReflectionLock::new(tmp.path()).unwrap();
        assert!(lock.acquire("other-session", "analysis").unwrap());
        let orchestrator = ReflectionOrchestrator::new("blocked-session", tmp.path());

        let report = orchestrator
            .run(&[assistant_msg("Nothing to analyze.")], &[], None)
            .unwrap();

        assert!(report.locked);
        assert_eq!(report.state, ReflectionState::Idle);
        assert!(report.lightweight.patterns.is_empty());
        assert!(
            report
                .lightweight
                .summary
                .contains("another reflection in progress")
        );
        assert!(report.error_analysis.is_none());
    }

    #[test]
    fn run_with_import_error_content_includes_contextual_error_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("import-error-session", tmp.path());

        let report = orchestrator
            .run(
                &[assistant_msg("Inspecting the command output.")],
                &[],
                Some("ImportError: cannot import name ToolRunner from amplihack.tools"),
            )
            .unwrap();
        let error_analysis = report.error_analysis.unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert_eq!(error_analysis.category, ErrorCategory::ImportError);
        assert!(!error_analysis.suggestions.is_empty());
    }

    #[test]
    fn absent_or_empty_error_content_skips_contextual_error_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let none_orchestrator = ReflectionOrchestrator::new("no-error-session", tmp.path());
        let empty_orchestrator = ReflectionOrchestrator::new("empty-error-session", tmp.path());
        let messages = vec![assistant_msg("No failures were reported.")];

        let none_report = none_orchestrator.run(&messages, &[], None).unwrap();
        let empty_report = empty_orchestrator.run(&messages, &[], Some("")).unwrap();

        assert_eq!(none_report.state, ReflectionState::Completed);
        assert!(none_report.error_analysis.is_none());
        assert_eq!(empty_report.state, ReflectionState::Completed);
        assert!(empty_report.error_analysis.is_none());
    }

    #[test]
    fn successful_run_persists_reloadable_completed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("persisted-session", tmp.path());

        orchestrator
            .run(
                &[assistant_msg("The operation finished cleanly.")],
                &[],
                None,
            )
            .unwrap();
        let state_machine = ReflectionStateMachine::new("persisted-session", tmp.path()).unwrap();
        let persisted = state_machine.read_state().unwrap();

        assert_eq!(persisted.state, ReflectionState::Completed);
        assert_eq!(persisted.session_id.as_deref(), Some("persisted-session"));
        assert!(persisted.analysis.is_some());
    }

    #[test]
    fn successful_run_releases_lock_for_later_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let first = ReflectionOrchestrator::new("first-session", tmp.path());
        let second = ReflectionOrchestrator::new("second-session", tmp.path());
        let messages = vec![assistant_msg("The operation finished cleanly.")];

        first.run(&messages, &[], None).unwrap();
        let second_report = second.run(&messages, &[], None).unwrap();

        assert!(!second_report.locked);
        assert_eq!(second_report.state, ReflectionState::Completed);
        assert!(!ReflectionLock::new(tmp.path()).unwrap().is_locked());
    }

    #[test]
    fn reflection_report_serialization_round_trips_required_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("serialized-session", tmp.path());

        let report = orchestrator
            .run(
                &[assistant_msg("Reviewing a permission failure.")],
                &[],
                Some("PermissionError: access denied while opening /tmp/example"),
            )
            .unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let restored: ReflectionReport = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.session_id, report.session_id);
        assert_eq!(restored.state, ReflectionState::AwaitingApproval);
        assert_eq!(restored.locked, report.locked);
        assert_eq!(
            restored.error_analysis.unwrap().category,
            ErrorCategory::Permission
        );
    }

    #[test]
    fn tool_logs_are_included_in_lightweight_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("tool-log-session", tmp.path());
        let tool_logs = vec!["cargo test failed with error code 101".to_string()];

        let report = orchestrator
            .run(&[assistant_msg("Checking test output.")], &tool_logs, None)
            .unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert!(
            report
                .lightweight
                .patterns
                .iter()
                .any(|pattern| pattern.kind == PatternKind::Error)
        );
    }

    #[test]
    fn contextual_error_analysis_drives_approval_even_with_clean_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("contextual-error-session", tmp.path());

        let report = orchestrator
            .run(
                &[assistant_msg("The latest response looked clean.")],
                &[],
                Some("FileNotFoundError: no such file or directory: config.yaml"),
            )
            .unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert!(report.lightweight.patterns.is_empty());
        assert_eq!(
            report.error_analysis.unwrap().category,
            ErrorCategory::FileMissing
        );
    }

    #[test]
    fn minimal_run_without_assistant_messages_completes_safely() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = ReflectionOrchestrator::new("minimal-session", tmp.path());

        let report = orchestrator
            .run(&[user_msg("Start reflection.")], &[], None)
            .unwrap();

        assert_eq!(report.state, ReflectionState::Completed);
        assert!(!report.locked);
        assert!(report.lightweight.patterns.is_empty());
        assert!(
            report
                .lightweight
                .summary
                .contains("Not enough messages to analyze")
        );
        assert!(report.error_analysis.is_none());
    }
}
