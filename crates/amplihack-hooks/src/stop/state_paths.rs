//! Generated Stop state, derived only from an already resolved session directory.

use std::path::PathBuf;

pub(super) struct StopStatePaths {
    pub(super) locks: PathBuf,
    pub(super) power_steering: PathBuf,
    pub(super) logs: PathBuf,
    pub(super) reflection: PathBuf,
    pub(super) session_key: String,
}

impl StopStatePaths {
    /// `session_dir` must come from `amplihack_types::session_state_dir`.
    pub(super) fn new(session_dir: PathBuf) -> Self {
        let session_key = session_dir
            .file_name()
            .expect("resolved session directory has a final component")
            .to_string_lossy()
            .into_owned();
        Self {
            locks: session_dir.join("locks"),
            power_steering: session_dir.join("power-steering"),
            logs: session_dir.join("logs"),
            reflection: session_dir.join("reflection"),
            session_key,
        }
    }

    pub(super) fn completion_marker(&self) -> PathBuf {
        self.power_steering
            .join(format!(".{}_completed", self.session_key))
    }

    pub(super) fn reflection_semaphore(&self) -> PathBuf {
        self.reflection
            .join(format!(".reflection_presented_{}", self.session_key))
    }

    pub(super) fn reflection_history(&self, timestamp: u64) -> PathBuf {
        self.reflection
            .join(format!("reflection_{}_{timestamp}.md", self.session_key))
    }
}
