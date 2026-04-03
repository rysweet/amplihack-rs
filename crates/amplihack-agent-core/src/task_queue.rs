//! Priority task queue for agent work items.
//!
//! Provides a simple priority-aware queue. Higher-priority tasks are
//! dequeued before lower-priority ones at the same insertion order.

use crate::error::{AgentError, Result};
use crate::models::{TaskPriority, TaskSpec};
use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// TaskQueue
// ---------------------------------------------------------------------------

/// A bounded priority queue for `TaskSpec` items.
pub struct TaskQueue {
    items: VecDeque<TaskSpec>,
    capacity: usize,
}

impl TaskQueue {
    /// Create a new queue with the given capacity (minimum 1).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            items: VecDeque::new(),
            capacity,
        }
    }

    /// Enqueue a task. Returns error if at capacity.
    pub fn enqueue(&mut self, task: TaskSpec) -> Result<()> {
        if self.is_full() {
            return Err(AgentError::QueueFull(format!(
                "queue is at capacity ({})",
                self.capacity
            )));
        }
        // Insert maintaining priority order (Critical first, then High, Normal, Low).
        // Within same priority, maintain FIFO by inserting at end of same-priority group.
        let pos = self
            .items
            .iter()
            .position(|t| t.priority < task.priority)
            .unwrap_or(self.items.len());
        self.items.insert(pos, task);
        Ok(())
    }

    /// Dequeue the highest-priority task (FIFO within same priority).
    pub fn dequeue(&mut self) -> Option<TaskSpec> {
        self.items.pop_front()
    }

    /// Peek at the next task without removing it.
    pub fn peek(&self) -> Option<&TaskSpec> {
        self.items.front()
    }

    /// Number of tasks in the queue.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.items.len() >= self.capacity
    }

    /// Remaining capacity.
    pub fn remaining(&self) -> usize {
        self.capacity.saturating_sub(self.items.len())
    }

    /// Remove all tasks from the queue.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Drain all tasks matching a priority level.
    pub fn drain_priority(&mut self, priority: TaskPriority) -> Vec<TaskSpec> {
        let mut drained = Vec::new();
        let mut remaining = VecDeque::new();
        for item in self.items.drain(..) {
            if item.priority == priority {
                drained.push(item);
            } else {
                remaining.push_back(item);
            }
        }
        self.items = remaining;
        drained
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new(100)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_starts_empty() {
        let q = TaskQueue::new(10);
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert!(!q.is_full());
        assert_eq!(q.remaining(), 10);
    }

    #[test]
    fn default_capacity() {
        let q = TaskQueue::default();
        assert_eq!(q.remaining(), 100);
    }

    #[test]
    fn priority_ordering() {
        let mut q = TaskQueue::new(10);
        q.enqueue(TaskSpec::new("low").with_priority(TaskPriority::Low)).unwrap();
        q.enqueue(TaskSpec::new("critical").with_priority(TaskPriority::Critical)).unwrap();
        q.enqueue(TaskSpec::new("normal").with_priority(TaskPriority::Normal)).unwrap();
        q.enqueue(TaskSpec::new("high").with_priority(TaskPriority::High)).unwrap();

        assert_eq!(q.dequeue().unwrap().description, "critical");
        assert_eq!(q.dequeue().unwrap().description, "high");
        assert_eq!(q.dequeue().unwrap().description, "normal");
        assert_eq!(q.dequeue().unwrap().description, "low");
    }

    #[test]
    fn capacity_rejection() {
        let mut q = TaskQueue::new(2);
        q.enqueue(TaskSpec::new("a")).unwrap();
        q.enqueue(TaskSpec::new("b")).unwrap();
        assert!(q.is_full());
        let err = q.enqueue(TaskSpec::new("c")).unwrap_err();
        assert!(matches!(err, AgentError::QueueFull(_)));
    }

    #[test]
    fn dequeue_empty_returns_none() {
        let mut q = TaskQueue::new(5);
        assert!(q.dequeue().is_none());
    }

    #[test]
    fn fifo_within_same_priority() {
        let mut q = TaskQueue::new(10);
        q.enqueue(TaskSpec::new("first").with_priority(TaskPriority::Normal)).unwrap();
        q.enqueue(TaskSpec::new("second").with_priority(TaskPriority::Normal)).unwrap();
        q.enqueue(TaskSpec::new("third").with_priority(TaskPriority::Normal)).unwrap();

        assert_eq!(q.dequeue().unwrap().description, "first");
        assert_eq!(q.dequeue().unwrap().description, "second");
        assert_eq!(q.dequeue().unwrap().description, "third");
    }

    #[test]
    fn drain_priority_removes_only_matching() {
        let mut q = TaskQueue::new(10);
        q.enqueue(TaskSpec::new("high1").with_priority(TaskPriority::High)).unwrap();
        q.enqueue(TaskSpec::new("normal1").with_priority(TaskPriority::Normal)).unwrap();
        q.enqueue(TaskSpec::new("high2").with_priority(TaskPriority::High)).unwrap();
        q.enqueue(TaskSpec::new("low1").with_priority(TaskPriority::Low)).unwrap();

        let drained = q.drain_priority(TaskPriority::High);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].description, "high1");
        assert_eq!(drained[1].description, "high2");
        assert_eq!(q.len(), 2);
        // Remaining should be normal and low
        assert_eq!(q.dequeue().unwrap().description, "normal1");
        assert_eq!(q.dequeue().unwrap().description, "low1");
    }

    #[test]
    fn drain_priority_empty_when_no_match() {
        let mut q = TaskQueue::new(10);
        q.enqueue(TaskSpec::new("normal").with_priority(TaskPriority::Normal)).unwrap();
        let drained = q.drain_priority(TaskPriority::Critical);
        assert!(drained.is_empty());
        assert_eq!(q.len(), 1);
    }
}
