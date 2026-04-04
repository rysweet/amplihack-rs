//! Network graph store — replicating GraphStore facade.
//!
//! Port of Python `amplihack/memory/network_store.py`.
//! Wraps a local `GraphStore` and replicates writes / merges searches
//! across a pluggable transport (local, Redis, or Azure Service Bus).
//! The actual transport is abstracted behind the `EventTransport` trait
//! so that the Rust crate compiles without Python dependencies.

use crate::graph_store::{EdgeDirection, EdgeRecord, GraphStore, Props};
use crate::network_store_types::merge_results;
use std::collections::{HashMap, HashSet};
use tracing::debug;

// Re-export types from the companion module.
pub use crate::network_store_types::{AgentRegistry, BusEvent, EventTransport, LocalTransport};

// ── Event type constants ──

const OP_CREATE_NODE: &str = "network_graph.create_node";
const OP_CREATE_EDGE: &str = "network_graph.create_edge";
const OP_SEARCH_QUERY: &str = "network_graph.search_query";
const OP_SEARCH_RESPONSE: &str = "network_graph.search_response";

/// Tables searched when handling inbound queries.
const QUERY_SEARCH_TABLES: &[&str] = &[
    "semantic_memory",
    "hive_facts",
    "episodic_memory",
    "general",
];

/// Network-replicating graph store.
///
/// Wraps a local `GraphStore`, replicates writes to peers via transport,
/// and merges remote search results with local hits.
pub struct NetworkGraphStore {
    agent_id: String,
    local: Box<dyn GraphStore>,
    transport: Box<dyn EventTransport>,
    registry: Option<AgentRegistry>,
    search_timeout_ms: u64,
    is_local_transport: bool,
}

impl NetworkGraphStore {
    pub fn new(
        agent_id: impl Into<String>,
        local: Box<dyn GraphStore>,
        transport: Box<dyn EventTransport>,
        is_local_transport: bool,
    ) -> anyhow::Result<Self> {
        let agent_id = agent_id.into();
        transport.subscribe(&agent_id)?;
        Ok(Self {
            agent_id,
            local,
            transport,
            registry: None,
            search_timeout_ms: 3000,
            is_local_transport,
        })
    }

    /// Attach a shared agent registry.
    pub fn with_registry(mut self, registry: AgentRegistry) -> Self {
        registry.register(&self.agent_id, HashMap::new());
        self.registry = Some(registry);
        self
    }

    /// Set search timeout in milliseconds.
    pub fn with_search_timeout(mut self, ms: u64) -> Self {
        self.search_timeout_ms = ms;
        self
    }

    /// Create a node locally and publish to peers.
    pub fn create_node(&mut self, table: &str, properties: &Props) -> anyhow::Result<String> {
        let node_id = self.local.create_node(table, properties)?;
        let mut payload = HashMap::new();
        payload.insert("table".into(), serde_json::json!(table));
        payload.insert("node_id".into(), serde_json::json!(node_id));
        payload.insert("properties".into(), serde_json::json!(properties));
        self.publish(OP_CREATE_NODE, payload);
        Ok(node_id)
    }

    /// Get a node from local store.
    pub fn get_node(&self, table: &str, node_id: &str) -> anyhow::Result<Option<Props>> {
        self.local.get_node(table, node_id)
    }

    /// Update node locally.
    pub fn update_node(
        &mut self,
        table: &str,
        node_id: &str,
        properties: &Props,
    ) -> anyhow::Result<()> {
        self.local.update_node(table, node_id, properties)
    }

    /// Delete node locally.
    pub fn delete_node(&mut self, table: &str, node_id: &str) -> anyhow::Result<()> {
        self.local.delete_node(table, node_id)
    }

    /// Query nodes from local store.
    pub fn query_nodes(
        &self,
        table: &str,
        filters: Option<&Props>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>> {
        self.local.query_nodes(table, filters, limit)
    }

    /// Search locally, then merge remote results (unless local transport).
    pub fn search_nodes(
        &mut self,
        table: &str,
        text: &str,
        fields: Option<&[&str]>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>> {
        let local_results = self.local.search_nodes(table, text, fields, limit)?;

        if self.is_local_transport {
            return Ok(local_results);
        }

        // Publish search query
        let query_id = uuid::Uuid::new_v4().to_string();
        let mut payload = HashMap::new();
        payload.insert("query_id".into(), serde_json::json!(query_id));
        payload.insert("table".into(), serde_json::json!(table));
        payload.insert("text".into(), serde_json::json!(text));
        payload.insert("limit".into(), serde_json::json!(limit));
        self.publish(OP_SEARCH_QUERY, payload);

        // Poll for responses up to timeout
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(self.search_timeout_ms);
        let mut remote_results: Vec<Props> = Vec::new();

        while std::time::Instant::now() < deadline {
            if let Ok(events) = self.transport.poll(&self.agent_id) {
                for event in events {
                    if event.event_type == OP_SEARCH_RESPONSE {
                        if let Some(qid) = event.payload.get("query_id")
                            && qid.as_str() == Some(&query_id)
                            && let Some(results) = event.payload.get("results")
                            && let Ok(items) = serde_json::from_value::<Vec<Props>>(results.clone())
                        {
                            remote_results.extend(items);
                        }
                    } else {
                        self.handle_event(&event);
                    }
                }
            }
            if !remote_results.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        Ok(merge_results(&local_results, &remote_results, limit))
    }

    /// Create edge locally and publish to peers.
    pub fn create_edge(
        &mut self,
        rel_type: &str,
        from_table: &str,
        from_id: &str,
        to_table: &str,
        to_id: &str,
        properties: &Props,
    ) -> anyhow::Result<()> {
        self.local
            .create_edge(rel_type, from_table, from_id, to_table, to_id, properties)?;
        let mut payload = HashMap::new();
        payload.insert("rel_type".into(), serde_json::json!(rel_type));
        payload.insert("from_table".into(), serde_json::json!(from_table));
        payload.insert("from_id".into(), serde_json::json!(from_id));
        payload.insert("to_table".into(), serde_json::json!(to_table));
        payload.insert("to_id".into(), serde_json::json!(to_id));
        payload.insert("properties".into(), serde_json::json!(properties));
        self.publish(OP_CREATE_EDGE, payload);
        Ok(())
    }

    /// Get edges from local store.
    pub fn get_edges(
        &self,
        node_id: &str,
        rel_type: Option<&str>,
        direction: EdgeDirection,
    ) -> anyhow::Result<Vec<EdgeRecord>> {
        self.local.get_edges(node_id, rel_type, direction)
    }

    /// Delete edge from local store.
    pub fn delete_edge(
        &mut self,
        rel_type: &str,
        from_id: &str,
        to_id: &str,
    ) -> anyhow::Result<()> {
        self.local.delete_edge(rel_type, from_id, to_id)
    }

    /// Process pending inbound events.
    pub fn process_events(&mut self) -> anyhow::Result<usize> {
        let events = self.transport.poll(&self.agent_id)?;
        let count = events.len();
        for event in &events {
            self.handle_event(event);
        }
        Ok(count)
    }

    /// Shut down transport and local store.
    pub fn close(&mut self) -> anyhow::Result<()> {
        if let Some(ref registry) = self.registry {
            registry.unregister(&self.agent_id);
        }
        let _ = self.transport.unsubscribe(&self.agent_id);
        let _ = self.transport.close();
        self.local.close()
    }

    // ── internals ──

    fn publish(&self, event_type: &str, payload: HashMap<String, serde_json::Value>) {
        let event = BusEvent {
            event_type: event_type.to_string(),
            source_agent: self.agent_id.clone(),
            payload,
        };
        if let Err(e) = self.transport.publish(&event) {
            debug!("failed to publish {event_type}: {e}");
        }
    }

    fn handle_event(&mut self, event: &BusEvent) {
        match event.event_type.as_str() {
            OP_CREATE_NODE => {
                let table = event
                    .payload
                    .get("table")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if table.is_empty() {
                    return;
                }
                if let Some(props) = event.payload.get("properties")
                    && let Ok(props_map) = serde_json::from_value::<Props>(props.clone())
                {
                    let node_id = props_map
                        .get("node_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !node_id.is_empty()
                        && self.local.get_node(table, node_id).ok().flatten().is_some()
                    {
                        return; // already have it
                    }
                    if let Err(e) = self.local.create_node(table, &props_map) {
                        debug!("remote create_node failed: {e}");
                    }
                }
            }
            OP_CREATE_EDGE => {
                let p = &event.payload;
                let rel = p.get("rel_type").and_then(|v| v.as_str()).unwrap_or("");
                let ft = p.get("from_table").and_then(|v| v.as_str()).unwrap_or("");
                let fi = p.get("from_id").and_then(|v| v.as_str()).unwrap_or("");
                let tt = p.get("to_table").and_then(|v| v.as_str()).unwrap_or("");
                let ti = p.get("to_id").and_then(|v| v.as_str()).unwrap_or("");
                let props: Props = p
                    .get("properties")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                if let Err(e) = self.local.create_edge(rel, ft, fi, tt, ti, &props) {
                    debug!("remote create_edge failed: {e}");
                }
            }
            OP_SEARCH_QUERY => {
                let qid = event
                    .payload
                    .get("query_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let table = event
                    .payload
                    .get("table")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let limit = event
                    .payload
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                if qid.is_empty() || table.is_empty() {
                    return;
                }

                let mut results = Vec::new();
                let mut seen: HashSet<String> = HashSet::new();

                let tables = std::iter::once(table)
                    .chain(QUERY_SEARCH_TABLES.iter().copied().filter(|t| *t != table));

                for t in tables {
                    if let Ok(hits) = self.local.search_nodes(t, text, None, limit) {
                        for h in hits {
                            let key = h
                                .get("node_id")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                                .unwrap_or_else(|| format!("{h:?}"));
                            if seen.insert(key) {
                                results.push(h);
                            }
                        }
                    }
                }

                let mut resp_payload = HashMap::new();
                resp_payload.insert("query_id".into(), serde_json::json!(qid));
                resp_payload.insert("results".into(), serde_json::json!(results));
                self.publish(OP_SEARCH_RESPONSE, resp_payload);
            }
            other => {
                debug!("unrecognised event type: {other}");
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/network_store_tests.rs"]
mod tests;
