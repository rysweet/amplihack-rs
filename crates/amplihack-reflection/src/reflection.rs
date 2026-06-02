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
    use crate::lightweight_analyzer::Role;

    fn assistant_msg(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: content.to_string(),
        }
    }

    fn user_msg(content: &str) -> Message {
        Message {
            role: Role::User,
            content: content.to_string(),
        }
    }

    #[test]
    fn orchestrator_new_stores_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-42", tmp.path());
        assert_eq!(orch.session_id, "sess-42");
        assert_eq!(orch.runtime_dir, tmp.path());
    }

    #[test]
    fn happy_path_no_patterns_completes() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-1", tmp.path());
        let msgs = vec![
            user_msg("hello"),
            assistant_msg("Hi there, how can I help?"),
        ];
        let report = orch.run(&msgs, &[], None).unwrap();

        assert_eq!(report.session_id, "sess-1");
        assert_eq!(report.state, ReflectionState::Completed);
        assert!(!report.locked);
        assert!(report.error_analysis.is_none());
        assert!(report.lightweight.patterns.is_empty());
    }

    #[test]
    fn patterns_detected_awaits_approval() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-2", tmp.path());
        let msgs = vec![
            user_msg("run build"),
            assistant_msg("The build failed with an error"),
        ];
        let report = orch.run(&msgs, &[], None).unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert!(!report.locked);
        assert!(!report.lightweight.patterns.is_empty());
    }

    #[test]
    fn lock_contention_returns_locked() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = ReflectionLock::new(tmp.path()).unwrap();
        assert!(lock.acquire("other-session", "test").unwrap());

        let orch = ReflectionOrchestrator::new("sess-3", tmp.path());
        let msgs = vec![assistant_msg("something")];
        let report = orch.run(&msgs, &[], None).unwrap();

        assert!(report.locked);
        assert_eq!(report.state, ReflectionState::Idle);
        assert_eq!(
            report.lightweight.summary,
            "Skipped \u{2014} another reflection in progress"
        );
        assert!(report.error_analysis.is_none());
    }

    #[test]
    fn error_content_produces_error_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-4", tmp.path());
        let msgs = vec![assistant_msg("checking output")];
        let report = orch
            .run(&msgs, &[], Some("ImportError: No module named foo"))
            .unwrap();

        assert!(report.error_analysis.is_some());
        assert!(!report.locked);
    }

    #[test]
    fn no_error_content_skips_error_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-5", tmp.path());
        let msgs = vec![assistant_msg("all good here")];
        let report = orch.run(&msgs, &[], None).unwrap();

        assert!(report.error_analysis.is_none());
    }

    #[test]
    fn empty_error_content_skips_error_analysis() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-6", tmp.path());
        let msgs = vec![assistant_msg("all good here")];
        let report = orch.run(&msgs, &[], Some("")).unwrap();

        assert!(report.error_analysis.is_none());
    }

    #[test]
    fn state_file_written_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-7", tmp.path());
        let msgs = vec![assistant_msg("ok")];
        orch.run(&msgs, &[], None).unwrap();

        let state_file = tmp.path().join("reflection_state_sess-7.json");
        assert!(state_file.exists(), "state file should be created");
        let data: ReflectionStateData =
            serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
        assert_eq!(data.state, ReflectionState::Completed);
        assert!(data.analysis.is_some());
    }

    #[test]
    fn lock_released_after_run() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-8", tmp.path());
        let msgs = vec![assistant_msg("ok")];
        orch.run(&msgs, &[], None).unwrap();

        let orch2 = ReflectionOrchestrator::new("sess-8b", tmp.path());
        let report2 = orch2.run(&msgs, &[], None).unwrap();
        assert!(!report2.locked);
    }

    #[test]
    fn report_serialization_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-9", tmp.path());
        let msgs = vec![
            user_msg("build"),
            assistant_msg("The build error was a timeout"),
        ];
        let report = orch
            .run(&msgs, &[], Some("ImportError: no module named bar"))
            .unwrap();

        let json = serde_json::to_string(&report).unwrap();
        let restored: ReflectionReport = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.session_id, report.session_id);
        assert_eq!(restored.state, report.state);
        assert_eq!(restored.locked, report.locked);
    }

    #[test]
    fn tool_logs_feed_into_analyzer() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-10", tmp.path());
        let msgs = vec![assistant_msg("checking")];
        let logs = vec!["Build failed with error code 1".to_string()];
        let report = orch.run(&msgs, &logs, None).unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert!(!report.lightweight.patterns.is_empty());
    }

    #[test]
    fn error_analysis_with_patterns_awaits_approval() {
        let tmp = tempfile::tempdir().unwrap();
        let orch = ReflectionOrchestrator::new("sess-11", tmp.path());
        let msgs = vec![assistant_msg("done")];
        let report = orch
            .run(
                &msgs,
                &[],
                Some("PermissionError: access denied to /etc/shadow"),
            )
            .unwrap();

        assert_eq!(report.state, ReflectionState::AwaitingApproval);
        assert!(report.error_analysis.is_some());
    }
}
