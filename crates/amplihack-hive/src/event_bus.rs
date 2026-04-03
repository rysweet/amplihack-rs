use std::collections::HashMap;

use crate::error::Result;
use crate::models::BusEvent;

/// Trait for publishing and consuming events within the hive.
pub trait EventBus {
    /// Publish an event to all subscribers of its topic.
    fn publish(&mut self, event: BusEvent) -> Result<()>;

    /// Subscribe a handler to a topic.
    fn subscribe(&mut self, topic: &str, handler_id: &str) -> Result<()>;

    /// Unsubscribe a handler from a topic.
    fn unsubscribe(&mut self, topic: &str, handler_id: &str) -> Result<()>;

    /// Return pending events for a handler without consuming them.
    fn pending_events(&self, handler_id: &str) -> Result<Vec<BusEvent>>;

    /// Consume and return all pending events for a handler.
    fn drain_events(&mut self, handler_id: &str) -> Result<Vec<BusEvent>>;
}

/// An in-process event bus backed by [`Vec`] queues.
#[allow(dead_code)] // Fields used once todo!() stubs are implemented
pub struct LocalEventBus {
    subscriptions: HashMap<String, Vec<String>>,
    queues: HashMap<String, Vec<BusEvent>>,
}

impl LocalEventBus {
    /// Create a new empty event bus.
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
            queues: HashMap::new(),
        }
    }
}

impl Default for LocalEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus for LocalEventBus {
    fn publish(&mut self, _event: BusEvent) -> Result<()> {
        todo!()
    }

    fn subscribe(&mut self, _topic: &str, _handler_id: &str) -> Result<()> {
        todo!()
    }

    fn unsubscribe(&mut self, _topic: &str, _handler_id: &str) -> Result<()> {
        todo!()
    }

    fn pending_events(&self, _handler_id: &str) -> Result<Vec<BusEvent>> {
        todo!()
    }

    fn drain_events(&mut self, _handler_id: &str) -> Result<Vec<BusEvent>> {
        todo!()
    }
}
