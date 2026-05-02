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
