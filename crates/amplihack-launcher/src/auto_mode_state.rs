//! Thread-safe shared state for auto mode.
//!
//! Matches Python `amplihack/launcher/auto_mode_state.py`:
//! - Mutex-protected shared state
//! - Snapshot for UI rendering
//! - Cost and turn tracking

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum log buffer size.
const MAX_LOG_SIZE: usize = 1000;

/// Thread-safe shared state between auto mode execution and UI.
pub struct AutoModeState {
    inner: Mutex<AutoModeStateInner>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AutoModeStateInner {
    session_id: String,
    start_time: f64,
    turn: u32,
    max_turns: u32,
    objective: String,
    todos: Vec<HashMap<String, String>>,
    logs: VecDeque<String>,
    costs: CostInfo,
    status: String,
    pause_requested: bool,
    kill_requested: bool,
}

/// Cost tracking information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost: f64,
}

/// Snapshot of current state for UI rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub session_id: String,
    pub start_time: f64,
    pub turn: u32,
    pub max_turns: u32,
    pub objective: String,
    pub todos: Vec<HashMap<String, String>>,
    pub logs: Vec<String>,
    pub costs: CostInfo,
    pub status: String,
    pub pause_requested: bool,
    pub kill_requested: bool,
}

impl AutoModeState {
    pub fn new(
        session_id: impl Into<String>,
        max_turns: u32,
        objective: impl Into<String>,
    ) -> Self {
        Self {
            inner: Mutex::new(AutoModeStateInner {
                session_id: session_id.into(),
                start_time: now_secs(),
                turn: 1,
                max_turns,
                objective: objective.into(),
                todos: Vec::new(),
                logs: VecDeque::with_capacity(MAX_LOG_SIZE),
                costs: CostInfo::default(),
                status: "running".to_string(),
                pause_requested: false,
                kill_requested: false,
            }),
        }
    }

    /// Get a thread-safe snapshot of current state.
    pub fn snapshot(&self) -> StateSnapshot {
        let inner = self.inner.lock().unwrap();
        StateSnapshot {
            session_id: inner.session_id.clone(),
            start_time: inner.start_time,
            turn: inner.turn,
            max_turns: inner.max_turns,
            objective: inner.objective.clone(),
            todos: inner.todos.clone(),
            logs: inner.logs.iter().cloned().collect(),
            costs: inner.costs.clone(),
            status: inner.status.clone(),
            pause_requested: inner.pause_requested,
            kill_requested: inner.kill_requested,
        }
    }

    /// Add a log message.
    pub fn add_log(&self, message: &str, timestamp: bool) {
        let mut inner = self.inner.lock().unwrap();
        let msg = if timestamp {
            let now = chrono::Local::now();
            format!("[{}] {message}", now.format("%H:%M:%S"))
        } else {
            message.to_string()
        };
        if inner.logs.len() >= MAX_LOG_SIZE {
            inner.logs.pop_front();
        }
        inner.logs.push_back(msg);
    }

    /// Update turn number.
    pub fn update_turn(&self, turn: u32) {
        self.inner.lock().unwrap().turn = turn;
    }

    /// Update todo list.
    pub fn update_todos(&self, todos: Vec<HashMap<String, String>>) {
        self.inner.lock().unwrap().todos = todos;
    }

    /// Accumulate cost info.
    pub fn update_costs(&self, input_tokens: u64, output_tokens: u64, estimated_cost: f64) {
        let mut inner = self.inner.lock().unwrap();
        inner.costs.input_tokens += input_tokens;
        inner.costs.output_tokens += output_tokens;
        inner.costs.estimated_cost += estimated_cost;
    }

    /// Update execution status.
    pub fn update_status(&self, status: &str) {
        self.inner.lock().unwrap().status = status.to_string();
    }

    /// Request execution to pause.
    pub fn request_pause(&self) {
        self.inner.lock().unwrap().pause_requested = true;
    }

    /// Clear pause request.
    pub fn clear_pause_request(&self) {
        self.inner.lock().unwrap().pause_requested = false;
    }

    /// Request execution to terminate.
    pub fn request_kill(&self) {
        self.inner.lock().unwrap().kill_requested = true;
    }

    /// Check if pause is requested.
    pub fn is_pause_requested(&self) -> bool {
        self.inner.lock().unwrap().pause_requested
    }

    /// Check if kill is requested.
    pub fn is_kill_requested(&self) -> bool {
        self.inner.lock().unwrap().kill_requested
    }

    /// Get current status.
    pub fn get_status(&self) -> String {
        self.inner.lock().unwrap().status.clone()
    }

    /// Get current turn.
    pub fn get_turn(&self) -> u32 {
        self.inner.lock().unwrap().turn
    }

    /// Get elapsed time in seconds.
    pub fn get_elapsed_time(&self) -> f64 {
        let start = self.inner.lock().unwrap().start_time;
        let elapsed = now_secs() - start;
        elapsed.max(0.0)
    }

    /// Get recent log messages.
    pub fn get_logs(&self, n: Option<usize>) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        match n {
            None => inner.logs.iter().cloned().collect(),
            Some(count) => inner.logs.iter().rev().take(count).rev().cloned().collect(),
        }
    }

    /// Get current todo list.
    pub fn get_todos(&self) -> Vec<HashMap<String, String>> {
        self.inner.lock().unwrap().todos.clone()
    }

    /// Get cost info.
    pub fn get_costs(&self) -> CostInfo {
        self.inner.lock().unwrap().costs.clone()
    }

    /// Set start time (for coordinator initialization).
    pub fn set_start_time(&self, t: f64) {
        self.inner.lock().unwrap().start_time = t;
    }

    /// Set objective.
    pub fn set_objective(&self, obj: &str) {
        self.inner.lock().unwrap().objective = obj.to_string();
    }

    /// Set max turns.
    pub fn set_max_turns(&self, max: u32) {
        self.inner.lock().unwrap().max_turns = max;
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_snapshot() {
        let state = AutoModeState::new("s1", 10, "Build something");
        let snap = state.snapshot();
        assert_eq!(snap.session_id, "s1");
        assert_eq!(snap.max_turns, 10);
        assert_eq!(snap.objective, "Build something");
        assert_eq!(snap.status, "running");
        assert_eq!(snap.turn, 1);
    }

    #[test]
    fn add_log_with_timestamp() {
        let state = AutoModeState::new("s1", 5, "test");
        state.add_log("hello", true);
        let logs = state.get_logs(None);
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("hello"));
        assert!(logs[0].contains("]")); // has timestamp brackets
    }

    #[test]
    fn add_log_without_timestamp() {
        let state = AutoModeState::new("s1", 5, "test");
        state.add_log("raw message", false);
        let logs = state.get_logs(None);
        assert_eq!(logs[0], "raw message");
    }

    #[test]
    fn log_buffer_limit() {
        let state = AutoModeState::new("s1", 5, "test");
        for i in 0..MAX_LOG_SIZE + 10 {
            state.add_log(&format!("msg-{i}"), false);
        }
        let logs = state.get_logs(None);
        assert_eq!(logs.len(), MAX_LOG_SIZE);
        // First messages should have been evicted
        assert!(logs[0].contains(&format!("msg-{}", 10)));
    }

    #[test]
    fn update_turn() {
        let state = AutoModeState::new("s1", 10, "test");
        state.update_turn(5);
        assert_eq!(state.get_turn(), 5);
    }

    #[test]
    fn update_status() {
        let state = AutoModeState::new("s1", 10, "test");
        state.update_status("completed");
        assert_eq!(state.get_status(), "completed");
    }

    #[test]
    fn update_costs_accumulates() {
        let state = AutoModeState::new("s1", 10, "test");
        state.update_costs(100, 50, 0.01);
        state.update_costs(200, 100, 0.02);
        let costs = state.get_costs();
        assert_eq!(costs.input_tokens, 300);
        assert_eq!(costs.output_tokens, 150);
        assert!((costs.estimated_cost - 0.03).abs() < 0.001);
    }

    #[test]
    fn pause_and_kill_requests() {
        let state = AutoModeState::new("s1", 10, "test");
        assert!(!state.is_pause_requested());
        assert!(!state.is_kill_requested());

        state.request_pause();
        assert!(state.is_pause_requested());

        state.clear_pause_request();
        assert!(!state.is_pause_requested());

        state.request_kill();
        assert!(state.is_kill_requested());
    }

    #[test]
    fn update_todos() {
        let state = AutoModeState::new("s1", 10, "test");
        let mut todo = HashMap::new();
        todo.insert("status".to_string(), "pending".to_string());
        todo.insert("content".to_string(), "Do thing".to_string());
        state.update_todos(vec![todo]);
        let todos = state.get_todos();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0]["status"], "pending");
    }

    #[test]
    fn get_logs_limited() {
        let state = AutoModeState::new("s1", 5, "test");
        for i in 0..10 {
            state.add_log(&format!("msg-{i}"), false);
        }
        let logs = state.get_logs(Some(3));
        assert_eq!(logs.len(), 3);
        assert!(logs[2].contains("msg-9"));
    }

    #[test]
    fn elapsed_time_positive() {
        let state = AutoModeState::new("s1", 10, "test");
        let elapsed = state.get_elapsed_time();
        assert!(elapsed >= 0.0);
    }
}
