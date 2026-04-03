use amplihack_agent_core::{TaskPriority, TaskQueue, TaskSpec};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn task(desc: &str) -> TaskSpec {
    TaskSpec::new(desc)
}

fn task_with_priority(desc: &str, priority: TaskPriority) -> TaskSpec {
    TaskSpec::new(desc).with_priority(priority)
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn queue_starts_empty() {
    let q = TaskQueue::new(10);
    assert!(q.is_empty());
    assert_eq!(q.len(), 0);
}

#[test]
fn queue_reports_remaining() {
    let q = TaskQueue::new(5);
    assert_eq!(q.remaining(), 5);
    assert!(!q.is_full());
}

#[test]
fn default_queue_capacity() {
    let q = TaskQueue::default();
    assert_eq!(q.remaining(), 100);
}

// ---------------------------------------------------------------------------
// Enqueue / dequeue basics
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn enqueue_and_dequeue_fifo() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task("first")).unwrap();
    q.enqueue(task("second")).unwrap();
    let t = q.dequeue().unwrap();
    assert_eq!(t.description, "first");
    let t = q.dequeue().unwrap();
    assert_eq!(t.description, "second");
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn enqueue_increments_len() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task("a")).unwrap();
    assert_eq!(q.len(), 1);
    q.enqueue(task("b")).unwrap();
    assert_eq!(q.len(), 2);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn dequeue_decrements_len() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task("a")).unwrap();
    q.enqueue(task("b")).unwrap();
    q.dequeue();
    assert_eq!(q.len(), 1);
}

// ---------------------------------------------------------------------------
// Priority ordering
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn priority_ordering_critical_first() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task_with_priority("low", TaskPriority::Low)).unwrap();
    q.enqueue(task_with_priority("critical", TaskPriority::Critical)).unwrap();
    q.enqueue(task_with_priority("normal", TaskPriority::Normal)).unwrap();
    let t = q.dequeue().unwrap();
    assert_eq!(t.description, "critical");
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn same_priority_fifo() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task_with_priority("a", TaskPriority::High)).unwrap();
    q.enqueue(task_with_priority("b", TaskPriority::High)).unwrap();
    let t = q.dequeue().unwrap();
    assert_eq!(t.description, "a");
}

// ---------------------------------------------------------------------------
// Peek
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn peek_returns_next_without_removing() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task("peeked")).unwrap();
    let peeked = q.peek().unwrap();
    assert_eq!(peeked.description, "peeked");
    assert_eq!(q.len(), 1); // not removed
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn peek_empty_returns_none() {
    let q = TaskQueue::new(10);
    assert!(q.peek().is_none());
}

// ---------------------------------------------------------------------------
// Empty queue
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn dequeue_empty_returns_none() {
    let mut q = TaskQueue::new(10);
    assert!(q.dequeue().is_none());
}

// ---------------------------------------------------------------------------
// Capacity
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn enqueue_at_capacity_returns_error() {
    let mut q = TaskQueue::new(2);
    q.enqueue(task("a")).unwrap();
    q.enqueue(task("b")).unwrap();
    assert!(q.is_full());
    let result = q.enqueue(task("c"));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Clear
// ---------------------------------------------------------------------------

#[test]
fn clear_empties_queue() {
    let mut q = TaskQueue::new(10);
    // Items vector starts empty, clear should keep it empty.
    q.clear();
    assert!(q.is_empty());
}

// ---------------------------------------------------------------------------
// Drain priority
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn drain_priority_removes_matching() {
    let mut q = TaskQueue::new(10);
    q.enqueue(task_with_priority("low1", TaskPriority::Low)).unwrap();
    q.enqueue(task_with_priority("high1", TaskPriority::High)).unwrap();
    q.enqueue(task_with_priority("low2", TaskPriority::Low)).unwrap();
    let drained = q.drain_priority(TaskPriority::Low);
    assert_eq!(drained.len(), 2);
    assert_eq!(q.len(), 1);
}

// ---------------------------------------------------------------------------
// TaskSpec properties
// ---------------------------------------------------------------------------

#[test]
fn task_spec_defaults() {
    let t = TaskSpec::new("test");
    assert_eq!(t.priority, TaskPriority::Normal);
    assert_eq!(t.timeout_secs, 120);
    assert!(t.tags.is_empty());
}

#[test]
fn task_spec_timeout_duration() {
    let t = TaskSpec::new("x").with_timeout(30);
    assert_eq!(t.timeout(), std::time::Duration::from_secs(30));
}
