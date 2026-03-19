//! Thread-safe shared state for auto mode execution and UI.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub type AutoModeTodo = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AutoModeCosts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoModeSnapshot {
    pub session_id: String,
    pub start_time: f64,
    pub turn: u32,
    pub max_turns: u32,
    pub objective: String,
    pub todos: Vec<AutoModeTodo>,
    pub logs: Vec<String>,
    pub costs: AutoModeCosts,
    pub status: String,
    pub pause_requested: bool,
    pub kill_requested: bool,
}

impl Default for AutoModeSnapshot {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            start_time: 0.0,
            turn: 1,
            max_turns: 10,
            objective: String::new(),
            todos: Vec::new(),
            logs: Vec::new(),
            costs: AutoModeCosts::default(),
            status: "running".to_string(),
            pause_requested: false,
            kill_requested: false,
        }
    }
}

#[derive(Debug)]
pub struct AutoModeState {
    inner: Mutex<AutoModeSnapshot>,
}

impl Default for AutoModeState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(AutoModeSnapshot::default()),
        }
    }
}

impl AutoModeState {
    pub fn new(
        session_id: impl Into<String>,
        max_turns: u32,
        objective: impl Into<String>,
    ) -> Self {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        Self {
            inner: Mutex::new(AutoModeSnapshot {
                session_id: session_id.into(),
                start_time,
                max_turns,
                objective: objective.into(),
                ..AutoModeSnapshot::default()
            }),
        }
    }

    pub fn snapshot(&self) -> AutoModeSnapshot {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn add_log(&self, message: impl Into<String>, timestamp: bool) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let message = message.into();
        if timestamp {
            let now = chrono::Local::now().format("[%H:%M:%S]");
            guard.logs.push(format!("{now} {message}"));
        } else {
            guard.logs.push(message);
        }
        if guard.logs.len() > 1000 {
            let overflow = guard.logs.len() - 1000;
            guard.logs.drain(0..overflow);
        }
    }

    pub fn update_turn(&self, turn: u32) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .turn = turn;
    }

    pub fn update_todos(&self, todos: Vec<AutoModeTodo>) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .todos = todos;
    }

    pub fn update_costs(&self, input_tokens: u64, output_tokens: u64, estimated_cost: f64) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.costs.input_tokens += input_tokens;
        guard.costs.output_tokens += output_tokens;
        guard.costs.estimated_cost += estimated_cost;
    }

    pub fn update_status(&self, status: impl Into<String>) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status = status.into();
    }

    pub fn request_pause(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pause_requested = true;
    }

    pub fn clear_pause_request(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pause_requested = false;
    }

    pub fn request_kill(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .kill_requested = true;
    }

    pub fn is_pause_requested(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pause_requested
    }

    pub fn is_kill_requested(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .kill_requested
    }

    pub fn status(&self) -> String {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status
            .clone()
    }

    pub fn turn(&self) -> u32 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .turn
    }

    pub fn elapsed_time(&self) -> f64 {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        (now - guard.start_time).max(0.0)
    }

    pub fn logs(&self, n: Option<usize>) -> Vec<String> {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match n {
            Some(limit) => guard
                .logs
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            None => guard.logs.clone(),
        }
    }

    pub fn todos(&self) -> Vec<AutoModeTodo> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .todos
            .clone()
    }

    pub fn costs(&self) -> AutoModeCosts {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .costs
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(status: &str, title: &str) -> AutoModeTodo {
        BTreeMap::from([
            ("status".to_string(), status.to_string()),
            ("title".to_string(), title.to_string()),
        ])
    }

    #[test]
    fn snapshot_clones_all_state() {
        let state = AutoModeState::new("session-1", 15, "Ship parity");
        state.update_turn(3);
        state.update_todos(vec![todo("in_progress", "Audit hooks")]);
        state.update_costs(10, 20, 0.15);
        state.update_status("paused");
        state.request_pause();
        state.request_kill();
        state.add_log("hello", false);

        let snapshot = state.snapshot();

        assert_eq!(snapshot.session_id, "session-1");
        assert_eq!(snapshot.turn, 3);
        assert_eq!(snapshot.max_turns, 15);
        assert_eq!(snapshot.objective, "Ship parity");
        assert_eq!(snapshot.todos.len(), 1);
        assert_eq!(snapshot.logs, vec!["hello"]);
        assert_eq!(snapshot.costs.input_tokens, 10);
        assert_eq!(snapshot.costs.output_tokens, 20);
        assert_eq!(snapshot.costs.estimated_cost, 0.15);
        assert_eq!(snapshot.status, "paused");
        assert!(snapshot.pause_requested);
        assert!(snapshot.kill_requested);
    }

    #[test]
    fn add_log_prefixes_timestamp_and_caps_history() {
        let state = AutoModeState::default();
        state.add_log("timed", true);
        for index in 0..1005 {
            state.add_log(format!("log-{index}"), false);
        }

        let logs = state.logs(None);

        assert!(logs[0].starts_with("log-5"));
        assert!(logs.last().unwrap().ends_with("log-1004"));
        assert_eq!(logs.len(), 1000);
    }

    #[test]
    fn update_costs_accumulates_values() {
        let state = AutoModeState::default();
        state.update_costs(100, 50, 0.25);
        state.update_costs(20, 5, 0.10);

        let costs = state.costs();

        assert_eq!(costs.input_tokens, 120);
        assert_eq!(costs.output_tokens, 55);
        assert!((costs.estimated_cost - 0.35).abs() < f64::EPSILON);
    }

    #[test]
    fn elapsed_time_clamps_negative_values() {
        let state = AutoModeState::default();
        {
            let mut guard = state
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.start_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                + 60.0;
        }

        assert_eq!(state.elapsed_time(), 0.0);
    }

    #[test]
    fn pause_and_kill_requests_round_trip() {
        let state = AutoModeState::default();

        state.request_pause();
        state.request_kill();
        assert!(state.is_pause_requested());
        assert!(state.is_kill_requested());

        state.clear_pause_request();
        assert!(!state.is_pause_requested());
    }
}
