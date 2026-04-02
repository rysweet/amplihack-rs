//! Persistent task queue for fleet work distribution.
//!
//! Matches Python `amplihack/fleet/fleet_tasks.py`:
//! - JSON-backed persistent queue
//! - Priority-based ordering
//! - Assignment tracking (which VM/agent is working on what)
//! - Status lifecycle: pending → assigned → completed/failed

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

/// Task status in the queue lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Assigned,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

/// Priority level for tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// A task in the fleet queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetTask {
    pub id: String,
    pub description: String,
    pub priority: Priority,
    pub status: TaskStatus,
    pub assigned_to: Option<String>,
    pub repo_path: Option<String>,
    pub branch: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
    pub result: Option<String>,
}

impl FleetTask {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        let now = now_secs();
        Self {
            id: id.into(),
            description: description.into(),
            priority: Priority::Normal,
            status: TaskStatus::Pending,
            assigned_to: None,
            repo_path: None,
            branch: None,
            created_at: now,
            updated_at: now,
            result: None,
        }
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_repo(mut self, repo_path: impl Into<String>) -> Self {
        self.repo_path = Some(repo_path.into());
        self
    }
}

/// Persistent task queue backed by a JSON file.
pub struct TaskQueue {
    tasks: Vec<FleetTask>,
    persist_path: Option<PathBuf>,
}

impl TaskQueue {
    /// Create a new in-memory task queue.
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            persist_path: None,
        }
    }

    /// Create a persistent task queue backed by a JSON file.
    pub fn with_persistence(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let tasks = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Self {
            tasks,
            persist_path: Some(path),
        })
    }

    /// Add a task to the queue.
    pub fn enqueue(&mut self, task: FleetTask) -> Result<()> {
        info!(id = %task.id, "Enqueued task");
        self.tasks.push(task);
        self.save()
    }

    /// Get the next pending task (highest priority first).
    pub fn next_pending(&self) -> Option<&FleetTask> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .max_by_key(|t| t.priority)
    }

    /// Assign a task to a VM/agent.
    pub fn assign(&mut self, task_id: &str, assignee: &str) -> Result<bool> {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TaskStatus::Assigned;
            task.assigned_to = Some(assignee.to_string());
            task.updated_at = now_secs();
            info!(id = task_id, assignee, "Task assigned");
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Mark a task as completed.
    pub fn complete(&mut self, task_id: &str, result: Option<&str>) -> Result<bool> {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TaskStatus::Completed;
            task.result = result.map(String::from);
            task.updated_at = now_secs();
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Mark a task as failed.
    pub fn fail(&mut self, task_id: &str, reason: &str) -> Result<bool> {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TaskStatus::Failed;
            task.result = Some(reason.to_string());
            task.updated_at = now_secs();
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get all tasks with a given status.
    pub fn by_status(&self, status: TaskStatus) -> Vec<&FleetTask> {
        self.tasks.iter().filter(|t| t.status == status).collect()
    }

    /// Total task count.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Get a task by ID.
    pub fn get(&self, id: &str) -> Option<&FleetTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn save(&self) -> Result<()> {
        if let Some(ref path) = self.persist_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(&self.tasks)?;
            std::fs::write(path, json)
                .with_context(|| format!("failed to write {}", path.display()))?;
            debug!(path = %path.display(), "Task queue persisted");
        }
        Ok(())
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_and_dequeue() {
        let mut q = TaskQueue::new();
        q.enqueue(FleetTask::new("t1", "Fix bug")).unwrap();
        q.enqueue(FleetTask::new("t2", "Add feature").with_priority(Priority::High))
            .unwrap();
        assert_eq!(q.len(), 2);
        let next = q.next_pending().unwrap();
        assert_eq!(next.id, "t2", "highest priority should be first");
    }

    #[test]
    fn assign_and_complete() {
        let mut q = TaskQueue::new();
        q.enqueue(FleetTask::new("t1", "Work")).unwrap();
        assert!(q.assign("t1", "vm-1").unwrap());
        assert_eq!(q.get("t1").unwrap().status, TaskStatus::Assigned);
        assert!(q.complete("t1", Some("done")).unwrap());
        assert_eq!(q.get("t1").unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn fail_task() {
        let mut q = TaskQueue::new();
        q.enqueue(FleetTask::new("t1", "Work")).unwrap();
        assert!(q.fail("t1", "build error").unwrap());
        assert_eq!(q.get("t1").unwrap().status, TaskStatus::Failed);
        assert_eq!(q.get("t1").unwrap().result.as_deref(), Some("build error"));
    }

    #[test]
    fn by_status_filter() {
        let mut q = TaskQueue::new();
        q.enqueue(FleetTask::new("t1", "A")).unwrap();
        q.enqueue(FleetTask::new("t2", "B")).unwrap();
        q.assign("t1", "vm-1").unwrap();
        assert_eq!(q.by_status(TaskStatus::Pending).len(), 1);
        assert_eq!(q.by_status(TaskStatus::Assigned).len(), 1);
    }

    #[test]
    fn persistence_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        {
            let mut q = TaskQueue::with_persistence(&path).unwrap();
            q.enqueue(FleetTask::new("t1", "Persistent task")).unwrap();
        }
        let q = TaskQueue::with_persistence(&path).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q.get("t1").unwrap().description, "Persistent task");
    }

    #[test]
    fn nonexistent_task_returns_false() {
        let mut q = TaskQueue::new();
        assert!(!q.assign("nope", "vm").unwrap());
        assert!(!q.complete("nope", None).unwrap());
        assert!(!q.fail("nope", "err").unwrap());
    }

    #[test]
    fn priority_ordering() {
        assert!(Priority::Critical > Priority::High);
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn task_builder() {
        let task = FleetTask::new("t1", "Build feature")
            .with_priority(Priority::Critical)
            .with_repo("/home/user/project");
        assert_eq!(task.priority, Priority::Critical);
        assert_eq!(task.repo_path.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn duplicate_enqueue_allowed() {
        let mut q = TaskQueue::new();
        q.enqueue(FleetTask::new("t1", "First")).unwrap();
        q.enqueue(FleetTask::new("t1", "Duplicate")).unwrap();
        assert_eq!(q.len(), 2, "queue allows duplicate IDs (Vec-backed)");
    }

    #[test]
    fn next_pending_skips_assigned_and_completed() {
        let mut q = TaskQueue::new();
        q.enqueue(FleetTask::new("t1", "First")).unwrap();
        q.enqueue(FleetTask::new("t2", "Second")).unwrap();
        q.enqueue(FleetTask::new("t3", "Third")).unwrap();
        q.assign("t1", "vm-1").unwrap();
        q.complete("t2", None).unwrap();
        let next = q.next_pending().unwrap();
        assert_eq!(next.id, "t3");
    }

    #[test]
    fn empty_queue_next_pending_returns_none() {
        let q = TaskQueue::new();
        assert!(q.next_pending().is_none());
    }

    #[test]
    fn persistence_survives_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        std::fs::write(&path, "not valid json {{{").unwrap();
        let q = TaskQueue::with_persistence(&path).unwrap();
        assert!(
            q.is_empty(),
            "corrupted file should fall back to empty queue"
        );
    }

    #[test]
    fn task_status_serialization_round_trip() {
        for status in [
            TaskStatus::Pending,
            TaskStatus::Assigned,
            TaskStatus::InProgress,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }
}
