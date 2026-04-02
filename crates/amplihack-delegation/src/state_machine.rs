use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::DelegationError;

// ---------------------------------------------------------------------------
// ProcessState
// ---------------------------------------------------------------------------

/// Lifecycle states of a delegation subprocess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    /// Initial state after construction.
    Created,
    /// Subprocess is being spawned.
    Starting,
    /// Subprocess is actively running.
    Running,
    /// Subprocess has exited; collecting results.
    Completing,
    /// Terminal: finished successfully.
    Completed,
    /// Terminal: finished with an error.
    Failed,
}

impl fmt::Display for ProcessState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Completing => "completing",
            Self::Completed => "completed",
            Self::Failed => "failed",
        };
        write!(f, "{s}")
    }
}

impl ProcessState {
    /// Returns the set of states this state may transition to.
    fn valid_targets(self) -> &'static [ProcessState] {
        match self {
            Self::Created => &[Self::Starting, Self::Failed],
            Self::Starting => &[Self::Running, Self::Failed],
            Self::Running => &[Self::Completing, Self::Failed],
            Self::Completing => &[Self::Completed, Self::Failed],
            Self::Completed | Self::Failed => &[],
        }
    }

    /// Whether this is a terminal state (no further transitions allowed).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

// ---------------------------------------------------------------------------
// State history entry
// ---------------------------------------------------------------------------

/// A recorded state change with its timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateHistoryEntry {
    /// The state that was entered.
    pub state: ProcessState,
    /// When the transition occurred.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// SubprocessStateMachine
// ---------------------------------------------------------------------------

/// Tracks subprocess lifecycle with validated transitions and timeout enforcement.
#[derive(Debug)]
pub struct SubprocessStateMachine {
    current_state: ProcessState,
    timeout_seconds: u64,
    start_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
    failure_reason: Option<String>,
    state_history: Vec<StateHistoryEntry>,
}

impl SubprocessStateMachine {
    /// Create a new state machine with the given timeout (in seconds).
    pub fn new(timeout_seconds: u64) -> Self {
        let now = Utc::now();
        Self {
            current_state: ProcessState::Created,
            timeout_seconds,
            start_time: None,
            end_time: None,
            failure_reason: None,
            state_history: vec![StateHistoryEntry {
                state: ProcessState::Created,
                timestamp: now,
            }],
        }
    }

    /// The default timeout used by the Python implementation (30 minutes).
    pub const DEFAULT_TIMEOUT: u64 = 1800;

    /// Current lifecycle state.
    pub fn current_state(&self) -> ProcessState {
        self.current_state
    }

    /// Optional failure reason if the machine is in `Failed`.
    pub fn failure_reason(&self) -> Option<&str> {
        self.failure_reason.as_deref()
    }

    /// Attempt to move to `new_state`. Returns an error on invalid transitions.
    pub fn transition_to(
        &mut self,
        new_state: ProcessState,
        error: Option<&str>,
    ) -> Result<(), DelegationError> {
        let valid = self.current_state.valid_targets();
        if !valid.contains(&new_state) {
            return Err(DelegationError::InvalidTransition {
                from: self.current_state,
                to: new_state,
            });
        }

        let now = Utc::now();

        // Record start/end bookkeeping.
        if new_state == ProcessState::Running && self.start_time.is_none() {
            self.start_time = Some(now);
        }
        if new_state.is_terminal() {
            self.end_time = Some(now);
        }
        if new_state == ProcessState::Failed {
            self.failure_reason = error.map(String::from);
        }

        self.current_state = new_state;
        self.state_history.push(StateHistoryEntry {
            state: new_state,
            timestamp: now,
        });

        Ok(())
    }

    /// Whether the subprocess is currently running.
    pub fn is_running(&self) -> bool {
        self.current_state == ProcessState::Running
    }

    /// Whether the subprocess completed (successfully or with failure).
    pub fn is_complete(&self) -> bool {
        self.current_state.is_terminal()
    }

    /// Whether the subprocess failed.
    pub fn has_failed(&self) -> bool {
        self.current_state == ProcessState::Failed
    }

    /// Returns `true` if the elapsed time since `Running` exceeds the timeout.
    pub fn check_timeout(&self) -> bool {
        if let Some(start) = self.start_time {
            let elapsed = Utc::now().signed_duration_since(start);
            elapsed.num_seconds() as u64 >= self.timeout_seconds
        } else {
            false
        }
    }

    /// Elapsed seconds since the subprocess entered `Running`, or 0.
    pub fn get_elapsed_time(&self) -> f64 {
        match self.start_time {
            Some(start) => {
                let end = self.end_time.unwrap_or_else(Utc::now);
                end.signed_duration_since(start).num_milliseconds() as f64 / 1000.0
            }
            None => 0.0,
        }
    }

    /// The recorded state history (read-only).
    pub fn state_history(&self) -> &[StateHistoryEntry] {
        &self.state_history
    }

    /// Duration spent in each state (in seconds).
    pub fn get_state_durations(&self) -> Vec<(ProcessState, f64)> {
        let mut durations = Vec::new();
        let history = &self.state_history;
        for i in 0..history.len() {
            let entry = &history[i];
            let next_ts = if i + 1 < history.len() {
                history[i + 1].timestamp
            } else {
                self.end_time.unwrap_or_else(Utc::now)
            };
            let secs = next_ts
                .signed_duration_since(entry.timestamp)
                .num_milliseconds() as f64
                / 1000.0;
            durations.push((entry.state, secs));
        }
        durations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_machine_starts_in_created() {
        let sm = SubprocessStateMachine::new(60);
        assert_eq!(sm.current_state(), ProcessState::Created);
        assert!(!sm.is_running());
        assert!(!sm.is_complete());
        assert!(!sm.has_failed());
    }

    #[test]
    fn happy_path_transitions() {
        let mut sm = SubprocessStateMachine::new(60);
        sm.transition_to(ProcessState::Starting, None).unwrap();
        sm.transition_to(ProcessState::Running, None).unwrap();
        assert!(sm.is_running());
        sm.transition_to(ProcessState::Completing, None).unwrap();
        sm.transition_to(ProcessState::Completed, None).unwrap();
        assert!(sm.is_complete());
        assert!(!sm.has_failed());
    }

    #[test]
    fn fail_from_any_non_terminal_state() {
        for start in [
            ProcessState::Created,
            ProcessState::Starting,
            ProcessState::Running,
            ProcessState::Completing,
        ] {
            let mut sm = SubprocessStateMachine::new(60);
            // Walk to the desired state.
            let path: &[ProcessState] = match start {
                ProcessState::Created => &[],
                ProcessState::Starting => &[ProcessState::Starting],
                ProcessState::Running => &[ProcessState::Starting, ProcessState::Running],
                ProcessState::Completing => &[
                    ProcessState::Starting,
                    ProcessState::Running,
                    ProcessState::Completing,
                ],
                _ => unreachable!(),
            };
            for &s in path {
                sm.transition_to(s, None).unwrap();
            }
            sm.transition_to(ProcessState::Failed, Some("oops"))
                .unwrap();
            assert!(sm.has_failed());
            assert_eq!(sm.failure_reason(), Some("oops"));
        }
    }

    #[test]
    fn invalid_transition_is_err() {
        let mut sm = SubprocessStateMachine::new(60);
        let err = sm.transition_to(ProcessState::Completed, None).unwrap_err();
        assert!(matches!(err, DelegationError::InvalidTransition { .. }));
    }

    #[test]
    fn terminal_states_reject_all_transitions() {
        let mut sm = SubprocessStateMachine::new(60);
        sm.transition_to(ProcessState::Starting, None).unwrap();
        sm.transition_to(ProcessState::Running, None).unwrap();
        sm.transition_to(ProcessState::Completing, None).unwrap();
        sm.transition_to(ProcessState::Completed, None).unwrap();

        assert!(sm.transition_to(ProcessState::Running, None).is_err());
        assert!(sm.transition_to(ProcessState::Failed, None).is_err());
    }

    #[test]
    fn state_history_recorded() {
        let mut sm = SubprocessStateMachine::new(60);
        sm.transition_to(ProcessState::Starting, None).unwrap();
        sm.transition_to(ProcessState::Running, None).unwrap();
        assert_eq!(sm.state_history().len(), 3); // Created + Starting + Running
        assert_eq!(sm.state_history()[0].state, ProcessState::Created);
        assert_eq!(sm.state_history()[1].state, ProcessState::Starting);
        assert_eq!(sm.state_history()[2].state, ProcessState::Running);
    }

    #[test]
    fn elapsed_time_zero_before_running() {
        let sm = SubprocessStateMachine::new(60);
        assert_eq!(sm.get_elapsed_time(), 0.0);
    }

    #[test]
    fn elapsed_time_positive_while_running() {
        let mut sm = SubprocessStateMachine::new(60);
        sm.transition_to(ProcessState::Starting, None).unwrap();
        sm.transition_to(ProcessState::Running, None).unwrap();
        // Should be non-negative (may be 0 on fast machines).
        assert!(sm.get_elapsed_time() >= 0.0);
    }

    #[test]
    fn check_timeout_false_before_running() {
        let sm = SubprocessStateMachine::new(1);
        assert!(!sm.check_timeout());
    }

    #[test]
    fn check_timeout_with_zero_timeout() {
        let mut sm = SubprocessStateMachine::new(0);
        sm.transition_to(ProcessState::Starting, None).unwrap();
        sm.transition_to(ProcessState::Running, None).unwrap();
        // With a 0-second timeout, should immediately report timeout.
        assert!(sm.check_timeout());
    }

    #[test]
    fn state_durations_computed() {
        let mut sm = SubprocessStateMachine::new(60);
        sm.transition_to(ProcessState::Starting, None).unwrap();
        sm.transition_to(ProcessState::Running, None).unwrap();
        sm.transition_to(ProcessState::Completing, None).unwrap();
        sm.transition_to(ProcessState::Completed, None).unwrap();
        let durations = sm.get_state_durations();
        assert_eq!(durations.len(), 5);
        // All durations should be non-negative.
        for (_state, d) in &durations {
            assert!(*d >= 0.0);
        }
    }

    #[test]
    fn process_state_display() {
        assert_eq!(ProcessState::Created.to_string(), "created");
        assert_eq!(ProcessState::Running.to_string(), "running");
        assert_eq!(ProcessState::Completed.to_string(), "completed");
        assert_eq!(ProcessState::Failed.to_string(), "failed");
    }

    #[test]
    fn process_state_is_terminal() {
        assert!(!ProcessState::Created.is_terminal());
        assert!(!ProcessState::Starting.is_terminal());
        assert!(!ProcessState::Running.is_terminal());
        assert!(!ProcessState::Completing.is_terminal());
        assert!(ProcessState::Completed.is_terminal());
        assert!(ProcessState::Failed.is_terminal());
    }

    #[test]
    fn default_timeout_constant() {
        assert_eq!(SubprocessStateMachine::DEFAULT_TIMEOUT, 1800);
    }
}
