//! [`DistributedHiveMind`] — top-level orchestrator for the distributed hive.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::event_bus::{EventBus, LocalEventBus};
use super::coordinator::HiveCoordinator;
use super::node::AgentNode;

/// System-level orchestrator managing agents, propagation, and query routing.
pub struct DistributedHiveMind {
    bus: Arc<Mutex<LocalEventBus>>,
    coordinator: Arc<Mutex<HiveCoordinator>>,
    agents: HashMap<String, AgentNode>,
}

impl DistributedHiveMind {
    pub fn new() -> Self {
        Self {
            bus: Arc::new(Mutex::new(LocalEventBus::new())),
            coordinator: Arc::new(Mutex::new(HiveCoordinator::new())),
            agents: HashMap::new(),
        }
    }

    pub fn with_bus(bus: LocalEventBus) -> Self {
        Self { bus: Arc::new(Mutex::new(bus)),
               coordinator: Arc::new(Mutex::new(HiveCoordinator::new())),
               agents: HashMap::new() }
    }

    pub fn create_agent(&mut self, agent_id: &str, domain: &str) -> &AgentNode {
        let mut node = AgentNode::new(agent_id, domain);
        node.join_hive(Arc::clone(&self.bus), Arc::clone(&self.coordinator));
        self.agents.insert(agent_id.to_string(), node);
        &self.agents[agent_id]
    }

    pub fn get_agent(&self, agent_id: &str) -> Option<&AgentNode> { self.agents.get(agent_id) }
    pub fn get_agent_mut(&mut self, agent_id: &str) -> Option<&mut AgentNode> { self.agents.get_mut(agent_id) }
    pub fn agent_count(&self) -> usize { self.agents.len() }

    pub fn propagate(&mut self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for (id, agent) in &mut self.agents {
            counts.insert(id.clone(), agent.process_pending_events());
        }
        counts
    }

    pub fn query_routed(&self, _asking: &str, query: &str, limit: usize) -> Vec<crate::models::HiveFact> {
        let experts = if let Ok(c) = self.coordinator.lock() { c.route_query(query) }
        else { return Vec::new(); };
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for eid in experts.into_iter().take(3) {
            if let Some(agent) = self.agents.get(&eid) {
                for fact in agent.query(query, limit) {
                    if seen.insert(fact.content.clone()) { results.push(fact); }
                }
            }
        }
        results.truncate(limit);
        results
    }

    pub fn query_all_agents(&self, query: &str, limit: usize) -> Vec<crate::models::HiveFact> {
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for agent in self.agents.values() {
            for fact in agent.query(query, limit) {
                if seen.insert(fact.content.clone()) { results.push(fact); }
            }
        }
        results.truncate(limit);
        results
    }

    pub fn remove_agent(&mut self, agent_id: &str) {
        if let Some(mut a) = self.agents.remove(agent_id) { a.leave_hive(); }
    }

    pub fn get_stats(&self) -> serde_json::Value {
        let stats: serde_json::Map<String, serde_json::Value> = self.agents.iter()
            .map(|(id, a)| (id.clone(), serde_json::json!({
                "fact_count": a.fact_count(), "domain": a.domain(), "connected": a.is_connected(),
            }))).collect();
        let coord = self.coordinator.lock().map(|c| c.get_hive_stats()).unwrap_or_default();
        serde_json::json!({ "agents": stats, "coordinator": coord })
    }

    pub fn close(&mut self) {
        let ids: Vec<String> = self.agents.keys().cloned().collect();
        for id in ids { self.remove_agent(&id); }
        if let Ok(mut bus) = self.bus.lock() { let _ = bus.close(); }
    }
}

impl Default for DistributedHiveMind { fn default() -> Self { Self::new() } }
impl Drop for DistributedHiveMind { fn drop(&mut self) { self.close(); } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_remove() {
        let mut hive = DistributedHiveMind::new();
        hive.create_agent("a1", "security");
        assert_eq!(hive.agent_count(), 1);
        hive.remove_agent("a1");
        assert_eq!(hive.agent_count(), 0);
    }

    #[test]
    fn propagation_shares_facts() {
        let mut hive = DistributedHiveMind::new();
        hive.create_agent("a1", "security");
        hive.create_agent("a2", "security");
        hive.get_agent_mut("a1").unwrap().learn("v", "SQL injection is dangerous", 0.9, None).unwrap();
        let counts = hive.propagate();
        assert_eq!(*counts.get("a2").unwrap_or(&0), 1);
    }

    #[test]
    fn query_routed_finds_expert() {
        let mut hive = DistributedHiveMind::new();
        hive.create_agent("a1", "security");
        hive.create_agent("a2", "networking");
        hive.get_agent_mut("a1").unwrap().learn("security", "XSS is a web vulnerability", 0.9, None).unwrap();
        assert!(!hive.query_routed("a2", "security", 10).is_empty());
    }

    #[test]
    fn query_all_agents_merges() {
        let mut hive = DistributedHiveMind::new();
        hive.create_agent("a1", "d");
        hive.create_agent("a2", "d");
        hive.get_agent_mut("a1").unwrap().learn("topic", "fact one", 0.9, None).unwrap();
        hive.get_agent_mut("a2").unwrap().learn("topic", "fact two", 0.8, None).unwrap();
        assert_eq!(hive.query_all_agents("topic", 10).len(), 2);
    }

    #[test]
    fn close_disconnects_all() {
        let mut hive = DistributedHiveMind::new();
        hive.create_agent("a1", "d");
        hive.close();
        assert_eq!(hive.agent_count(), 0);
    }
}
