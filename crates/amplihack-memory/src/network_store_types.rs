//! Types and transports for the network graph store.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Lightweight bus event envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEvent {
    pub event_type: String,
    pub source_agent: String,
    pub payload: HashMap<String, serde_json::Value>,
}

/// Transport abstraction — backends plug in at runtime.
pub trait EventTransport: Send + Sync {
    fn publish(&self, event: &BusEvent) -> anyhow::Result<()>;
    fn poll(&self, agent_id: &str) -> anyhow::Result<Vec<BusEvent>>;
    fn subscribe(&self, agent_id: &str) -> anyhow::Result<()>;
    fn unsubscribe(&self, agent_id: &str) -> anyhow::Result<()>;
    fn close(&self) -> anyhow::Result<()>;
}

/// In-process local transport (for testing and single-agent mode).
pub struct LocalTransport {
    queues: Mutex<HashMap<String, Vec<BusEvent>>>,
}

impl LocalTransport {
    pub fn new() -> Self {
        Self {
            queues: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for LocalTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl EventTransport for LocalTransport {
    fn publish(&self, event: &BusEvent) -> anyhow::Result<()> {
        let mut queues = self.queues.lock().unwrap();
        for (agent_id, queue) in queues.iter_mut() {
            if *agent_id != event.source_agent {
                queue.push(event.clone());
            }
        }
        Ok(())
    }

    fn poll(&self, agent_id: &str) -> anyhow::Result<Vec<BusEvent>> {
        let mut queues = self.queues.lock().unwrap();
        let events = queues
            .get_mut(agent_id)
            .map(std::mem::take)
            .unwrap_or_default();
        Ok(events)
    }

    fn subscribe(&self, agent_id: &str) -> anyhow::Result<()> {
        self.queues
            .lock()
            .unwrap()
            .entry(agent_id.to_string())
            .or_default();
        Ok(())
    }

    fn unsubscribe(&self, agent_id: &str) -> anyhow::Result<()> {
        self.queues.lock().unwrap().remove(agent_id);
        Ok(())
    }

    fn close(&self) -> anyhow::Result<()> {
        self.queues.lock().unwrap().clear();
        Ok(())
    }
}

/// Thread-safe agent registry for peer discovery.
#[derive(Debug, Default, Clone)]
pub struct AgentRegistry {
    agents: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, agent_id: &str, metadata: HashMap<String, String>) {
        self.agents
            .lock()
            .unwrap()
            .insert(agent_id.to_string(), metadata);
    }

    pub fn unregister(&self, agent_id: &str) {
        self.agents.lock().unwrap().remove(agent_id);
    }

    pub fn list_agents(&self) -> Vec<String> {
        self.agents.lock().unwrap().keys().cloned().collect()
    }

    pub fn get(&self, agent_id: &str) -> Option<HashMap<String, String>> {
        self.agents.lock().unwrap().get(agent_id).cloned()
    }
}

/// Merge and deduplicate local + remote results by `node_id`.
pub fn merge_results(
    local: &[HashMap<String, serde_json::Value>],
    remote: &[HashMap<String, serde_json::Value>],
    limit: usize,
) -> Vec<HashMap<String, serde_json::Value>> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut merged = Vec::new();
    for node in local.iter().chain(remote.iter()) {
        let key = node
            .get("node_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| serde_json::to_string(node).unwrap_or_default());
        if seen.insert(key) {
            merged.push(node.clone());
            if merged.len() >= limit {
                break;
            }
        }
    }
    merged
}
