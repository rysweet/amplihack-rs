use std::collections::{HashMap, HashSet};

use tracing::warn;

use crate::error::Result;
use crate::models::BusEvent;

/// Maximum number of events a single agent mailbox can hold.
pub const MAX_MAILBOX_SIZE: usize = 1_000_000;

/// Trait for publishing and consuming events within the hive.
pub trait EventBus {
    /// Publish an event to all subscribers whose filters match.
    fn publish(&mut self, event: BusEvent) -> Result<()>;

    /// Subscribe an agent, optionally filtering by event types.
    fn subscribe(&mut self, agent_id: &str, event_types: Option<&[&str]>) -> Result<()>;

    /// Unsubscribe an agent from all topics.
    fn unsubscribe(&mut self, agent_id: &str) -> Result<()>;

    /// Return pending events for an agent without consuming them.
    fn pending_events(&self, agent_id: &str) -> Result<Vec<BusEvent>>;

    /// Consume and return all pending events for an agent (destructive drain).
    fn poll(&mut self, agent_id: &str) -> Result<Vec<BusEvent>>;

    /// Close the bus, clearing all state.
    fn close(&mut self) -> Result<()>;
}

struct Subscription {
    event_types: Option<HashSet<String>>,
}

/// An in-process event bus with per-agent mailboxes and event-type filtering.
pub struct LocalEventBus {
    subscriptions: HashMap<String, Subscription>,
    mailboxes: HashMap<String, Vec<BusEvent>>,
    closed: bool,
}

impl LocalEventBus {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
            mailboxes: HashMap::new(),
            closed: false,
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
        if self.closed {
            return Ok(());
        }
        let agent_ids: Vec<String> = self.subscriptions.keys().cloned().collect();
        for agent_id in agent_ids {
            let sub = &self.subscriptions[&agent_id];
            let matches = match &sub.event_types {
                Some(types) => types.contains(&event.topic),
                None => true,
            };
            if matches {
                let mailbox = self.mailboxes.entry(agent_id.clone()).or_default();
                if mailbox.len() >= MAX_MAILBOX_SIZE {
                    warn!(agent_id = %agent_id, "Mailbox at capacity ({MAX_MAILBOX_SIZE}), dropping oldest");
                    mailbox.remove(0);
                }
                mailbox.push(event.clone());
            }
        }
        Ok(())
    }

    fn subscribe(&mut self, agent_id: &str, event_types: Option<&[&str]>) -> Result<()> {
        let types = event_types.map(|t| t.iter().map(|s| s.to_string()).collect());
        self.subscriptions
            .insert(agent_id.to_string(), Subscription { event_types: types });
        self.mailboxes.entry(agent_id.to_string()).or_default();
        Ok(())
    }

    fn unsubscribe(&mut self, agent_id: &str) -> Result<()> {
        self.subscriptions.remove(agent_id);
        self.mailboxes.remove(agent_id);
        Ok(())
    }

    fn pending_events(&self, agent_id: &str) -> Result<Vec<BusEvent>> {
        Ok(self.mailboxes.get(agent_id).cloned().unwrap_or_default())
    }

    fn poll(&mut self, agent_id: &str) -> Result<Vec<BusEvent>> {
        Ok(self.mailboxes.remove(agent_id).unwrap_or_default())
    }

    fn close(&mut self) -> Result<()> {
        self.subscriptions.clear();
        self.mailboxes.clear();
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::make_event;

    fn ev(topic: &str, source: &str) -> BusEvent {
        make_event(topic, source, serde_json::json!({"k": "v"}))
    }

    #[test]
    fn publish_to_subscriber() {
        let mut bus = LocalEventBus::new();
        bus.subscribe("h1", None).unwrap();
        bus.publish(ev("t", "other")).unwrap();
        assert_eq!(bus.pending_events("h1").unwrap().len(), 1);
    }

    #[test]
    fn event_type_filter() {
        let mut bus = LocalEventBus::new();
        bus.subscribe("h1", Some(&["alpha"])).unwrap();
        bus.publish(ev("alpha", "src")).unwrap();
        bus.publish(ev("beta", "src")).unwrap();
        assert_eq!(bus.pending_events("h1").unwrap().len(), 1);
    }

    #[test]
    fn poll_drains() {
        let mut bus = LocalEventBus::new();
        bus.subscribe("h1", None).unwrap();
        bus.publish(ev("t", "src")).unwrap();
        let events = bus.poll("h1").unwrap();
        assert_eq!(events.len(), 1);
        assert!(bus.pending_events("h1").unwrap().is_empty());
    }

    #[test]
    fn close_clears_all() {
        let mut bus = LocalEventBus::new();
        bus.subscribe("h1", None).unwrap();
        bus.publish(ev("t", "src")).unwrap();
        bus.close().unwrap();
        assert!(bus.pending_events("h1").unwrap().is_empty());
        bus.publish(ev("t2", "src")).unwrap();
        assert!(bus.pending_events("h1").unwrap().is_empty());
    }

    #[test]
    fn unsubscribe_removes_agent() {
        let mut bus = LocalEventBus::new();
        bus.subscribe("h1", None).unwrap();
        bus.unsubscribe("h1").unwrap();
        bus.publish(ev("t", "src")).unwrap();
        assert!(bus.pending_events("h1").unwrap().is_empty());
    }
}
