//! Pluggable shard transport — [`ShardTransport`] trait with local and event-bus impls.

use crate::dht::{DHTRouter, ShardFact};
use crate::event_bus::{EventBus, LocalEventBus};
use crate::models::make_event;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Abstraction for routing shard queries/stores across the distributed hive.
pub trait ShardTransport {
    fn query_shard(&self, agent_id: &str, query: &str, limit: usize) -> Vec<ShardFact>;
    fn retrieve_by_entity_shard(
        &self,
        agent_id: &str,
        entity: &str,
        limit: usize,
    ) -> Vec<ShardFact>;
    fn store_on_shard(&mut self, agent_id: &str, fact: ShardFact);
    fn execute_aggregation_shard(
        &self,
        agent_id: &str,
        query_type: &str,
        entity_filter: &str,
    ) -> HashMap<String, serde_json::Value>;
}

/// In-process transport that accesses the DHT router directly.
pub struct LocalShardTransport {
    router: Arc<Mutex<DHTRouter>>,
}

impl LocalShardTransport {
    pub fn new(router: Arc<Mutex<DHTRouter>>) -> Self {
        Self { router }
    }
}

impl ShardTransport for LocalShardTransport {
    fn query_shard(&self, agent_id: &str, query: &str, limit: usize) -> Vec<ShardFact> {
        let r = match self.router.lock() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        r.get_shard(agent_id)
            .map(|s| s.search(query, limit).into_iter().cloned().collect())
            .unwrap_or_default()
    }
    fn retrieve_by_entity_shard(
        &self,
        agent_id: &str,
        entity: &str,
        limit: usize,
    ) -> Vec<ShardFact> {
        self.query_shard(agent_id, entity, limit)
    }
    fn store_on_shard(&mut self, _agent_id: &str, fact: ShardFact) {
        if let Ok(mut r) = self.router.lock() {
            r.store_fact(fact);
        }
    }
    fn execute_aggregation_shard(
        &self,
        agent_id: &str,
        query_type: &str,
        _entity_filter: &str,
    ) -> HashMap<String, serde_json::Value> {
        let r = match self.router.lock() {
            Ok(r) => r,
            Err(_) => return HashMap::new(),
        };
        let shard = match r.get_shard(agent_id) {
            Some(s) => s,
            None => return HashMap::new(),
        };
        let mut result = HashMap::new();
        match query_type {
            "count_total" => {
                result.insert("count".into(), serde_json::json!(shard.fact_count()));
            }
            "list_concepts" => {
                let concepts: Vec<String> = shard
                    .all_facts()
                    .iter()
                    .map(|f| f.concept.clone())
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                result.insert("concepts".into(), serde_json::json!(concepts));
            }
            _ => {}
        }
        result
    }
}

/// Correlation-based RPC transport over the hive event bus.
pub struct EventBusShardTransport {
    bus: Arc<Mutex<LocalEventBus>>,
    agent_id: String,
    local_router: Option<Arc<Mutex<DHTRouter>>>,
}

impl EventBusShardTransport {
    pub fn new(bus: Arc<Mutex<LocalEventBus>>, agent_id: impl Into<String>) -> Self {
        Self {
            bus,
            agent_id: agent_id.into(),
            local_router: None,
        }
    }
    pub fn with_local_router(mut self, router: Arc<Mutex<DHTRouter>>) -> Self {
        self.local_router = Some(router);
        self
    }
    fn is_local(&self, agent_id: &str) -> bool {
        self.local_router.is_some() && agent_id == self.agent_id
    }
    fn local_query(&self, query: &str, limit: usize) -> Vec<ShardFact> {
        if let Some(router) = &self.local_router
            && let Ok(r) = router.lock()
        {
            return r
                .get_shard(&self.agent_id)
                .map(|s| s.search(query, limit).into_iter().cloned().collect())
                .unwrap_or_default();
        }
        Vec::new()
    }
    fn publish_shard_event(&self, topic: &str, payload: serde_json::Value) {
        let event = make_event(topic, &self.agent_id, payload);
        if let Ok(mut bus) = self.bus.lock() {
            let _ = bus.publish(event);
        }
    }
}

impl ShardTransport for EventBusShardTransport {
    fn query_shard(&self, agent_id: &str, query: &str, limit: usize) -> Vec<ShardFact> {
        if self.is_local(agent_id) {
            return self.local_query(query, limit);
        }
        self.publish_shard_event(
            "SHARD_QUERY",
            serde_json::json!({
                "correlation_id": uuid::Uuid::new_v4().to_string(),
                "target_agent": agent_id, "operation": "search",
                "query": query, "limit": limit,
            }),
        );
        Vec::new()
    }
    fn retrieve_by_entity_shard(
        &self,
        agent_id: &str,
        entity: &str,
        limit: usize,
    ) -> Vec<ShardFact> {
        self.query_shard(agent_id, entity, limit)
    }
    fn store_on_shard(&mut self, agent_id: &str, fact: ShardFact) {
        if self.is_local(agent_id)
            && let Some(router) = &self.local_router
            && let Ok(mut r) = router.lock()
        {
            r.store_fact(fact);
            return;
        }
        self.publish_shard_event(
            "SHARD_STORE",
            serde_json::json!({
                "target_agent": agent_id, "fact": serde_json::to_value(&fact).unwrap_or_default(),
            }),
        );
    }
    fn execute_aggregation_shard(
        &self,
        agent_id: &str,
        query_type: &str,
        entity_filter: &str,
    ) -> HashMap<String, serde_json::Value> {
        if self.is_local(agent_id)
            && let Some(router) = &self.local_router
            && let Ok(r) = router.lock()
            && let Some(shard) = r.get_shard(agent_id)
        {
            let mut result = HashMap::new();
            if query_type == "count_total" {
                result.insert("count".into(), serde_json::json!(shard.fact_count()));
            }
            return result;
        }
        self.publish_shard_event(
            "SHARD_AGGREGATION",
            serde_json::json!({
                "target_agent": agent_id, "query_type": query_type,
                "entity_filter": entity_filter,
            }),
        );
        HashMap::new()
    }
}

/// Handle an incoming SHARD_STORE event by storing the fact locally.
#[cfg(test)]
pub(crate) fn handle_shard_store(event: &crate::models::BusEvent, router: &Mutex<DHTRouter>) {
    if let Some(fv) = event.payload.get("fact")
        && let Ok(fact) = serde_json::from_value::<ShardFact>(fv.clone())
        && let Ok(mut r) = router.lock()
    {
        r.store_fact(fact);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_router() -> Arc<Mutex<DHTRouter>> {
        let mut r = DHTRouter::new(2, 5);
        r.add_agent("a1");
        r.add_agent("a2");
        Arc::new(Mutex::new(r))
    }

    #[test]
    fn local_transport_query() {
        let router = make_router();
        {
            let mut r = router.lock().unwrap();
            r.store_fact(ShardFact::new("f1", "Rust is memory-safe"));
        }
        let t = LocalShardTransport::new(Arc::clone(&router));
        let r1 = t.query_shard("a1", "Rust", 10);
        let r2 = t.query_shard("a2", "Rust", 10);
        assert!(r1.len() + r2.len() >= 1);
    }

    #[test]
    fn local_transport_aggregation() {
        let router = make_router();
        {
            let mut r = router.lock().unwrap();
            r.store_fact(ShardFact::new("f1", "some fact content"));
        }
        let t = LocalShardTransport::new(Arc::clone(&router));
        let agg = t.execute_aggregation_shard("a1", "count_total", "");
        assert!(agg.contains_key("count"));
    }

    #[test]
    fn event_bus_transport_remote_publishes() {
        let bus = Arc::new(Mutex::new(LocalEventBus::new()));
        {
            let mut b = bus.lock().unwrap();
            b.subscribe("listener", Some(&["SHARD_QUERY"])).unwrap();
        }
        let t = EventBusShardTransport::new(Arc::clone(&bus), "a1");
        let _ = t.query_shard("a2", "test query", 10);
        let events = bus.lock().unwrap().poll("listener").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].topic, "SHARD_QUERY");
    }

    #[test]
    fn handle_shard_store_event() {
        let router = make_router();
        let fact = ShardFact::new("f1", "stored via event");
        let event = make_event(
            "SHARD_STORE",
            "a2",
            serde_json::json!({"fact": serde_json::to_value(&fact).unwrap()}),
        );
        handle_shard_store(&event, &router);
        let r = router.lock().unwrap();
        let total: usize = r
            .all_agents()
            .iter()
            .filter_map(|a| r.get_shard(a))
            .map(|s| s.fact_count())
            .sum();
        assert!(total >= 1);
    }
}
