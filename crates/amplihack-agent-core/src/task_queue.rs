//! Priority task queue for agent work items.
//!
//! Provides a simple priority-aware queue. Higher-priority tasks are
//! dequeued before lower-priority ones at the same insertion order.

use crate::error::{AgentError, Result};
use crate::models::{TaskPriority, TaskSpec};

// ---------------------------------------------------------------------------
// TaskQueue
// ---------------------------------------------------------------------------

/// A bounded priority queue for `TaskSpec` items.
pub struct TaskQueue {
    items: Vec<TaskSpec>,
    capacity: usize,
}

impl TaskQueue {
    /// Create a new queue with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            items: Vec::new(),
            capacity,
        }
    }

    /// Enqueue a task. Returns error if at capacity.
    pub fn enqueue(&mut self, task: TaskSpec) -> Result<()> {
        if self.is_full() {
            return Err(AgentError::TaskFailed("queue is at capacity".into()));
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
        if self.items.is_empty() {
            None
        } else {
            Some(self.items.remove(0))
        }
    }

    /// Peek at the next task without removing it.
    pub fn peek(&self) -> Option<&TaskSpec> {
        self.items.first()
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
        let mut remaining = Vec::new();
        for item in self.items.drain(..) {
            if item.priority == priority {
                drained.push(item);
            } else {
                remaining.push(item);
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
}
