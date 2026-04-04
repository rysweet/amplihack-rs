//! Thread coordinator for auto mode background execution.
//!
//! Matches Python `amplihack/launcher/auto_mode_coordinator.py`:
//! - Manages auto mode execution in a background thread
//! - Lifecycle management (start, monitor, wait)
//! - Thread-safe state updates via `AutoModeState`

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::auto_mode_state::AutoModeState;

/// Trait for auto mode execution.
pub trait AutoModeRunner: Send + 'static {
    /// Run auto mode execution. Returns exit code.
    fn run(&mut self) -> i32;
    /// Get the max turns configured.
    fn max_turns(&self) -> u32;
    /// Get the prompt/objective.
    fn prompt(&self) -> &str;
}

/// Type alias for state callback.
pub type StateCallback = Box<dyn Fn(&AutoModeState) + Send + Sync>;

/// Manages auto mode execution in a background thread.
pub struct AutoModeCoordinator {
    state: Arc<AutoModeState>,
    execution_thread: Option<thread::JoinHandle<()>>,
    state_callback: Option<StateCallback>,
}

impl AutoModeCoordinator {
    pub fn new(state: Arc<AutoModeState>, state_callback: Option<StateCallback>) -> Self {
        Self {
            state,
            execution_thread: None,
            state_callback,
        }
    }

    /// Start auto mode execution in background thread.
    pub fn start(&mut self, mut runner: impl AutoModeRunner) -> anyhow::Result<()> {
        if self.is_alive() {
            anyhow::bail!("Auto mode already running");
        }

        // Initialize state
        let max_turns = runner.max_turns();
        let prompt = runner.prompt().to_string();
        self.state.set_max_turns(max_turns);
        self.state.set_objective(&prompt);
        self.state.update_turn(1);
        self.state.update_status("running");
        self.state
            .add_log(&format!("Auto mode started (max {max_turns} turns)"), true);

        let state = Arc::clone(&self.state);
        let callback = self
            .state_callback
            .as_ref()
            .map(|_| Arc::clone(&self.state));

        let handle = thread::Builder::new()
            .name("AutoModeExecution".into())
            .spawn(move || {
                let exit_code = runner.run();

                if exit_code == 0 {
                    state.update_status("completed");
                    state.add_log("Auto mode completed successfully", true);
                } else {
                    state.update_status("error");
                    state.add_log(&format!("Auto mode exited with code {exit_code}"), true);
                }

                drop(callback);
            })?;

        self.execution_thread = Some(handle);
        Ok(())
    }

    /// Wait for execution thread to complete.
    pub fn wait(&mut self, timeout: Option<std::time::Duration>) {
        if let Some(handle) = self.execution_thread.take() {
            match timeout {
                Some(dur) => {
                    let start = Instant::now();
                    while start.elapsed() < dur {
                        if handle.is_finished() {
                            let _ = handle.join();
                            return;
                        }
                        thread::sleep(std::time::Duration::from_millis(50));
                    }
                    // Timeout — put handle back
                    self.execution_thread = Some(handle);
                }
                None => {
                    let _ = handle.join();
                }
            }
        }
    }

    /// Check if execution thread is alive.
    pub fn is_alive(&self) -> bool {
        self.execution_thread
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};

    struct MockRunner {
        exit_code: Arc<AtomicI32>,
        max: u32,
        prompt: String,
    }

    impl AutoModeRunner for MockRunner {
        fn run(&mut self) -> i32 {
            self.exit_code.load(Ordering::Relaxed)
        }
        fn max_turns(&self) -> u32 {
            self.max
        }
        fn prompt(&self) -> &str {
            &self.prompt
        }
    }

    #[test]
    fn start_and_complete() {
        let state = Arc::new(AutoModeState::new("s1", 5, "test"));
        let mut coord = AutoModeCoordinator::new(Arc::clone(&state), None);

        let runner = MockRunner {
            exit_code: Arc::new(AtomicI32::new(0)),
            max: 5,
            prompt: "Build something".into(),
        };

        coord.start(runner).unwrap();
        coord.wait(Some(std::time::Duration::from_secs(5)));

        assert_eq!(state.get_status(), "completed");
    }

    #[test]
    fn start_with_error_exit() {
        let state = Arc::new(AutoModeState::new("s2", 3, "test"));
        let mut coord = AutoModeCoordinator::new(Arc::clone(&state), None);

        let runner = MockRunner {
            exit_code: Arc::new(AtomicI32::new(1)),
            max: 3,
            prompt: "Fail".into(),
        };

        coord.start(runner).unwrap();
        coord.wait(None);

        assert_eq!(state.get_status(), "error");
    }

    #[test]
    fn cannot_start_twice() {
        let state = Arc::new(AutoModeState::new("s3", 5, "test"));
        let mut coord = AutoModeCoordinator::new(Arc::clone(&state), None);

        // Use a runner that blocks briefly
        struct SlowRunner;
        impl AutoModeRunner for SlowRunner {
            fn run(&mut self) -> i32 {
                thread::sleep(std::time::Duration::from_millis(500));
                0
            }
            fn max_turns(&self) -> u32 {
                5
            }
            fn prompt(&self) -> &str {
                "test"
            }
        }

        coord.start(SlowRunner).unwrap();
        assert!(coord.start(SlowRunner).is_err());

        coord.wait(None);
    }

    #[test]
    fn is_alive_before_and_after() {
        let state = Arc::new(AutoModeState::new("s4", 5, "test"));
        let mut coord = AutoModeCoordinator::new(Arc::clone(&state), None);

        assert!(!coord.is_alive());

        let runner = MockRunner {
            exit_code: Arc::new(AtomicI32::new(0)),
            max: 5,
            prompt: "test".into(),
        };

        coord.start(runner).unwrap();
        // May or may not be alive (race), so just check after wait
        coord.wait(None);
        assert!(!coord.is_alive());
    }
}
