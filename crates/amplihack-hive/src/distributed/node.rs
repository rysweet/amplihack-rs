//! [`AgentNode`] — individual agent with local fact storage and event-bus integration.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::event_bus::{EventBus, LocalEventBus};
use crate::graph::HiveGraph;
use crate::models::{BusEvent, HiveFact, PEER_CONFIDENCE_DISCOUNT, make_event};

use super::MAX_INCORPORATED_EVENTS;
use super::coordinator::HiveCoordinator;

/// An individual agent node that stores facts locally and participates in
/// the distributed hive via an event bus.
pub struct AgentNode {
    agent_id: String,
    domain: String,
    graph: HiveGraph,
    bus: Option<Arc<Mutex<LocalEventBus>>>,
    coordinator: Option<Arc<Mutex<HiveCoordinator>>>,
    incorporated_ids: VecDeque<String>,
    incorporated_set: HashSet<String>,
}

impl AgentNode {
    pub fn new(agent_id: impl Into<String>, domain: impl Into<String>) -> Self {
        let id: String = agent_id.into();
        Self {
            graph: HiveGraph::with_id(format!("agent-graph-{id}")),
            agent_id: id,
            domain: domain.into(),
            bus: None,
            coordinator: None,
            incorporated_ids: VecDeque::new(),
            incorporated_set: HashSet::new(),
        }
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
    pub fn domain(&self) -> &str {
        &self.domain
    }
    pub fn is_connected(&self) -> bool {
        self.bus.is_some()
    }
    pub fn fact_count(&self) -> usize {
        self.graph.fact_count()
    }
    pub fn graph(&self) -> &HiveGraph {
        &self.graph
    }

    /// Store a fact locally, publish FACT_LEARNED event, report to coordinator.
    pub fn learn(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: Option<Vec<String>>,
    ) -> crate::error::Result<String> {
        let fact_id = self.graph.store_fact(
            concept,
            content,
            confidence,
            &self.agent_id,
            tags.unwrap_or_default(),
        )?;
        if let Some(bus) = &self.bus {
            let payload = serde_json::json!({
                "fact_id": fact_id, "concept": concept, "content": content,
                "confidence": confidence, "source_agent": self.agent_id,
            });
            if let Ok(mut bus) = bus.lock() {
                let _ = bus.publish(make_event("FACT_LEARNED", &self.agent_id, payload));
            }
        }
        if let Some(coord) = &self.coordinator
            && let Ok(mut coord) = coord.lock()
        {
            coord.report_fact(&self.agent_id, concept);
        }
        Ok(fact_id)
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<HiveFact> {
        self.graph
            .query_facts(query, 0.0, limit)
            .unwrap_or_default()
    }

    pub fn get_all_facts(&self, limit: usize) -> Vec<HiveFact> {
        self.graph.all_facts().into_iter().take(limit).collect()
    }

    /// Incorporate a peer fact with confidence discount and dedup.
    pub fn incorporate_peer_fact(&mut self, event: &BusEvent) -> bool {
        if self.incorporated_set.contains(&event.event_id) {
            return false;
        }
        if event.source_id == self.agent_id {
            return false;
        }
        let concept = event
            .payload
            .get("concept")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = event
            .payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let confidence = event
            .payload
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);
        if content.is_empty() {
            return false;
        }
        let discounted = confidence * PEER_CONFIDENCE_DISCOUNT;
        let source = event
            .payload
            .get("source_agent")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tags = vec![format!("peer_source:{source}")];
        let ok = self
            .graph
            .store_fact(concept, content, discounted, &self.agent_id, tags)
            .is_ok();
        if ok {
            self.track_incorporated(&event.event_id);
        }
        ok
    }

    pub fn join_hive(
        &mut self,
        bus: Arc<Mutex<LocalEventBus>>,
        coordinator: Arc<Mutex<HiveCoordinator>>,
    ) {
        if let Ok(mut b) = bus.lock() {
            let _ = b.subscribe(&self.agent_id, Some(&["FACT_LEARNED", "FACT_PROMOTED"]));
        }
        if let Ok(mut c) = coordinator.lock() {
            c.register_agent(&self.agent_id, &self.domain);
        }
        self.bus = Some(bus);
        self.coordinator = Some(coordinator);
    }

    pub fn leave_hive(&mut self) {
        if let Some(bus) = self.bus.take()
            && let Ok(mut b) = bus.lock()
        {
            let _ = b.unsubscribe(&self.agent_id);
        }
        if let Some(coord) = self.coordinator.take()
            && let Ok(mut c) = coord.lock()
        {
            c.unregister_agent(&self.agent_id);
        }
    }

    pub fn process_pending_events(&mut self) -> usize {
        let events = if let Some(bus) = &self.bus {
            bus.lock()
                .ok()
                .and_then(|mut b| b.poll(&self.agent_id).ok())
                .unwrap_or_default()
        } else {
            return 0;
        };
        let mut count = 0;
        for event in &events {
            if self.incorporate_peer_fact(event) {
                count += 1;
            }
        }
        count
    }

    fn track_incorporated(&mut self, event_id: &str) {
        if self.incorporated_set.len() >= MAX_INCORPORATED_EVENTS
            && let Some(oldest) = self.incorporated_ids.pop_front()
        {
            self.incorporated_set.remove(&oldest);
        }
        let id = event_id.to_string();
        self.incorporated_set.insert(id.clone());
        self.incorporated_ids.push_back(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_agent_node() {
        let node = AgentNode::new("agent-1", "security");
        assert_eq!(node.agent_id(), "agent-1");
        assert!(!node.is_connected());
        assert_eq!(node.fact_count(), 0);
    }

    #[test]
    fn learn_stores_locally() {
        let mut node = AgentNode::new("a1", "research");
        let fid = node
            .learn("rust", "Rust is memory-safe", 0.9, None)
            .unwrap();
        assert!(!fid.is_empty());
        assert_eq!(node.fact_count(), 1);
    }

    #[test]
    fn query_returns_matching_facts() {
        let mut node = AgentNode::new("a1", "lang");
        node.learn("rust", "Rust ownership model", 0.9, None)
            .unwrap();
        node.learn("python", "Python is interpreted", 0.8, None)
            .unwrap();
        assert!(!node.query("rust", 10).is_empty());
    }

    #[test]
    fn incorporate_peer_fact_applies_discount() {
        let mut node = AgentNode::new("a1", "d");
        let event = make_event(
            "FACT_LEARNED",
            "a2",
            serde_json::json!({"concept":"c","content":"some info","confidence":1.0,"source_agent":"a2"}),
        );
        assert!(node.incorporate_peer_fact(&event));
        let facts = node.get_all_facts(10);
        assert_eq!(facts.len(), 1);
        assert!((facts[0].confidence - PEER_CONFIDENCE_DISCOUNT).abs() < 0.01);
    }

    #[test]
    fn incorporate_dedup_rejects_duplicate() {
        let mut node = AgentNode::new("a1", "d");
        let event = make_event(
            "FACT_LEARNED",
            "a2",
            serde_json::json!({"concept":"c","content":"fact","confidence":0.8,"source_agent":"a2"}),
        );
        assert!(node.incorporate_peer_fact(&event));
        assert!(!node.incorporate_peer_fact(&event));
    }

    #[test]
    fn incorporate_skips_own_events() {
        let mut node = AgentNode::new("a1", "d");
        let event = make_event(
            "FACT_LEARNED",
            "a1",
            serde_json::json!({"concept":"c","content":"fact","confidence":0.8,"source_agent":"a1"}),
        );
        assert!(!node.incorporate_peer_fact(&event));
    }

    #[test]
    fn join_and_leave_hive() {
        let bus = Arc::new(Mutex::new(LocalEventBus::new()));
        let coord = Arc::new(Mutex::new(HiveCoordinator::new()));
        let mut node = AgentNode::new("a1", "d");
        node.join_hive(bus, coord);
        assert!(node.is_connected());
        node.leave_hive();
        assert!(!node.is_connected());
    }

    #[test]
    fn bounded_dedup_evicts_oldest() {
        let mut node = AgentNode::new("a1", "d");
        for i in 0..MAX_INCORPORATED_EVENTS + 10 {
            node.track_incorporated(&format!("ev-{i}"));
        }
        assert_eq!(node.incorporated_set.len(), MAX_INCORPORATED_EVENTS);
    }

    #[test]
    fn process_pending_events_drains_bus() {
        let bus = Arc::new(Mutex::new(LocalEventBus::new()));
        let coord = Arc::new(Mutex::new(HiveCoordinator::new()));
        let mut a1 = AgentNode::new("a1", "d");
        let mut a2 = AgentNode::new("a2", "d");
        a1.join_hive(Arc::clone(&bus), Arc::clone(&coord));
        a2.join_hive(Arc::clone(&bus), Arc::clone(&coord));
        a1.learn("topic", "shared knowledge", 0.8, None).unwrap();
        let incorporated = a2.process_pending_events();
        assert_eq!(incorporated, 1);
        assert_eq!(a2.fact_count(), 1);
    }
}
