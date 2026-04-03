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
    fn publish(&mut self, event: BusEvent) -> Result<()> {
        if let Some(handlers) = self.subscriptions.get(&event.topic) {
            let handler_ids: Vec<String> = handlers.to_vec();
            for handler_id in handler_ids {
                self.queues
                    .entry(handler_id)
                    .or_default()
                    .push(event.clone());
            }
        }
        Ok(())
    }

    fn subscribe(&mut self, topic: &str, handler_id: &str) -> Result<()> {
        let handlers = self.subscriptions.entry(topic.to_string()).or_default();
        if !handlers.contains(&handler_id.to_string()) {
            handlers.push(handler_id.to_string());
        }
        Ok(())
    }

    fn unsubscribe(&mut self, topic: &str, handler_id: &str) -> Result<()> {
        if let Some(handlers) = self.subscriptions.get_mut(topic) {
            handlers.retain(|h| h != handler_id);
        }
        Ok(())
    }

    fn pending_events(&self, handler_id: &str) -> Result<Vec<BusEvent>> {
        Ok(self.queues.get(handler_id).cloned().unwrap_or_default())
    }

    fn drain_events(&mut self, handler_id: &str) -> Result<Vec<BusEvent>> {
        Ok(self.queues.remove(handler_id).unwrap_or_default())
    }
}
