//! `AutoModeRunner` — the "dumb" [`AgentSession`] half of the auto-mode split.
//!
//! It owns the [`PromptExecutor`] and does exactly one thing per turn: hand the
//! prompt verbatim to the executor and map its result into the generic
//! `amplihack_turn` turn types. All phase/loop control lives in
//! [`AutoModeChannel`](super::channel::AutoModeChannel).
//!
//! Mapping contract (behaviour-preserving):
//!   * `Ok(ExecutionResult { exit_code, stdout, .. })` — a subprocess that RAN,
//!     regardless of its exit code — becomes
//!     `Ok(TurnOutput::from_text(stdout).with_exit_code(exit_code))`.
//!   * `Err(_)` — a failure to RUN the executor at all — becomes
//!     `Err(TurnError::Exec(..))`, preserving the underlying error text.
//!
//! A ran-but-non-zero subprocess is therefore a normal `Ok` turn; only an
//! inability to run is a `TurnError`. This is the distinction that lets the
//! channel reproduce the old "required turn crashes vs. execute warns &
//! continues" behaviour purely from the exit code.

use super::{AutoModeTool, PromptExecutor};
use amplihack_turn::{AgentSession, TurnError, TurnOutput, TurnResult};
use std::path::PathBuf;

pub(super) struct AutoModeRunner<E: PromptExecutor> {
    executor: E,
    tool: AutoModeTool,
    execution_dir: PathBuf,
    project_dir: PathBuf,
    passthrough_args: Vec<String>,
    session_id: String,
}

impl<E: PromptExecutor> AutoModeRunner<E> {
    pub(super) fn new(
        executor: E,
        tool: AutoModeTool,
        execution_dir: PathBuf,
        project_dir: PathBuf,
        passthrough_args: Vec<String>,
        session_id: String,
    ) -> Self {
        Self {
            executor,
            tool,
            execution_dir,
            project_dir,
            passthrough_args,
            session_id,
        }
    }

    /// Borrow the wrapped executor (used by integration tests to assert the
    /// exact prompt sequence that reached the executor).
    #[cfg(test)]
    pub(super) fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: PromptExecutor> AgentSession for AutoModeRunner<E> {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        match self.executor.run_prompt(
            self.tool,
            &self.execution_dir,
            &self.project_dir,
            &self.passthrough_args,
            prompt,
        ) {
            // A subprocess that RAN (any exit code) is a normal turn.
            Ok(result) => Ok(TurnOutput::from_text(result.stdout).with_exit_code(result.exit_code)),
            // A failure to RUN the executor surfaces as a turn error, verbatim.
            Err(error) => Err(TurnError::Exec(format!("{error:#}"))),
        }
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }
}
