//! Builder for `ClaudeProcess`. Split into its own module so the main
//! `claude_process.rs` stays under the per-file 500-line cap.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::claude_process::{BuildError, ClaudeProcess, ProcessRunner, TokioProcessRunner};

#[derive(Default)]
pub struct ClaudeProcessBuilder {
    pub(crate) prompt: Option<String>,
    pub(crate) process_id: Option<String>,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) log_dir: Option<PathBuf>,
    pub(crate) model: Option<String>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) runner: Option<Arc<dyn ProcessRunner>>,
}

impl ClaudeProcessBuilder {
    pub fn prompt(mut self, p: impl Into<String>) -> Self {
        self.prompt = Some(p.into());
        self
    }
    pub fn process_id(mut self, id: impl Into<String>) -> Self {
        self.process_id = Some(id.into());
        self
    }
    pub fn working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = Some(dir);
        self
    }
    pub fn log_dir(mut self, dir: PathBuf) -> Self {
        self.log_dir = Some(dir);
        self
    }
    pub fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
    pub fn runner(mut self, r: Arc<dyn ProcessRunner>) -> Self {
        self.runner = Some(r);
        self
    }

    pub fn build(self) -> Result<ClaudeProcess, BuildError> {
        let prompt = self.prompt.ok_or(BuildError::MissingField("prompt"))?;
        let process_id = self
            .process_id
            .ok_or(BuildError::MissingField("process_id"))?;
        let working_dir = self
            .working_dir
            .ok_or(BuildError::MissingField("working_dir"))?;
        let log_dir = self.log_dir.ok_or(BuildError::MissingField("log_dir"))?;
        let runner = self
            .runner
            .unwrap_or_else(|| Arc::new(TokioProcessRunner::new()) as Arc<dyn ProcessRunner>);
        let _ = std::fs::create_dir_all(&log_dir);
        Ok(ClaudeProcess::__from_builder_parts(
            prompt,
            process_id,
            working_dir,
            log_dir,
            self.model,
            self.timeout,
            runner,
        ))
    }
}
