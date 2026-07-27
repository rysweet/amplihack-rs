//! GraphStore trait — abstract backend interface.
//!
//! Matches Python `amplihack/memory/graph_store.py`:
//! - 12 required methods (node CRUD, edge CRUD, schema, export/import)
//! - Cognitive memory table schemas

use std::collections::{HashMap, HashSet};

/// Properties map used by graph store operations.
pub type Props = HashMap<String, serde_json::Value>;

/// A single node: (table, node_id, properties).
pub type NodeTriple = (String, String, Props);

/// A single edge: (rel_type, from_id, to_id, properties).
pub type EdgeQuad = (String, String, String, Props);

/// Direction for edge queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
    Both,
}

/// Edge record returned by `get_edges`.
#[derive(Debug, Clone)]
pub struct EdgeRecord {
    pub rel_type: String,
    pub from_id: String,
    pub to_id: String,
    pub properties: Props,
}

/// Abstract graph store backend.
///
/// All memory backends must implement this trait to be usable with the
/// coordinator and distributed store.
pub trait GraphStore: Send + Sync {
    // ── Node operations ──

    fn create_node(&mut self, table: &str, properties: &Props) -> anyhow::Result<String>;
    fn get_node(&self, table: &str, node_id: &str) -> anyhow::Result<Option<Props>>;
    fn update_node(&mut self, table: &str, node_id: &str, properties: &Props)
    -> anyhow::Result<()>;
    fn delete_node(&mut self, table: &str, node_id: &str) -> anyhow::Result<()>;
    fn query_nodes(
        &self,
        table: &str,
        filters: Option<&Props>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>>;
    fn search_nodes(
        &self,
        table: &str,
        text: &str,
        fields: Option<&[&str]>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>>;

    // ── Edge operations ──

    fn create_edge(
        &mut self,
        rel_type: &str,
        from_table: &str,
        from_id: &str,
        to_table: &str,
        to_id: &str,
        properties: &Props,
    ) -> anyhow::Result<()>;
    fn get_edges(
        &self,
        node_id: &str,
        rel_type: Option<&str>,
        direction: EdgeDirection,
    ) -> anyhow::Result<Vec<EdgeRecord>>;
    fn delete_edge(&mut self, rel_type: &str, from_id: &str, to_id: &str) -> anyhow::Result<()>;

    // ── Schema operations ──

    fn ensure_table(&mut self, table: &str, schema: &Props) -> anyhow::Result<()>;

    // ── Export / Import ──

    fn get_all_node_ids(&self, table: Option<&str>) -> anyhow::Result<HashSet<String>>;
    fn export_nodes(&self, node_ids: Option<&[String]>) -> anyhow::Result<Vec<NodeTriple>>;
    fn export_edges(&self, node_ids: Option<&[String]>) -> anyhow::Result<Vec<EdgeQuad>>;
    fn import_nodes(&mut self, nodes: &[NodeTriple]) -> anyhow::Result<usize>;
    fn import_edges(&mut self, edges: &[EdgeQuad]) -> anyhow::Result<usize>;

    // ── Lifecycle ──

    fn close(&mut self) -> anyhow::Result<()>;
}
