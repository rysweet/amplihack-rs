//! In-memory graph store implementation.
//!
//! Matches Python `amplihack/memory/memory_store.py`:
//! - Dict-based node/edge storage
//! - Thread-safe via Mutex
//! - Text search across specified fields
//! - Export/import for gossip protocol

use crate::graph_store::{EdgeDirection, EdgeQuad, EdgeRecord, GraphStore, NodeTriple, Props};
use std::collections::{HashMap, HashSet};

/// In-memory graph store backed by HashMaps.
pub struct InMemoryGraphStore {
    /// table_name -> { node_id -> properties }
    nodes: HashMap<String, HashMap<String, Props>>,
    /// (rel_type, from_id, to_id) -> properties
    edges: Vec<(String, String, String, Props)>,
    next_id: u64,
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            next_id: 0,
        }
    }

    fn gen_id(&mut self) -> String {
        self.next_id += 1;
        format!("mem-{}", self.next_id)
    }

    fn text_matches(props: &Props, text_lower: &str, fields: Option<&[&str]>) -> bool {
        let check_field = |key: &str| -> bool {
            props
                .get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.to_lowercase().contains(text_lower))
        };
        match fields {
            Some(fs) => fs.iter().any(|f| check_field(f)),
            None => props.keys().any(|k| check_field(k)),
        }
    }
}

impl Default for InMemoryGraphStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphStore for InMemoryGraphStore {
    fn create_node(&mut self, table: &str, properties: &Props) -> anyhow::Result<String> {
        let id = properties
            .get("id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| self.gen_id());
        let mut props = properties.clone();
        props.insert("id".into(), serde_json::json!(id));
        self.nodes
            .entry(table.to_string())
            .or_default()
            .insert(id.clone(), props);
        Ok(id)
    }

    fn get_node(&self, table: &str, node_id: &str) -> anyhow::Result<Option<Props>> {
        Ok(self.nodes.get(table).and_then(|t| t.get(node_id)).cloned())
    }

    fn update_node(
        &mut self,
        table: &str,
        node_id: &str,
        properties: &Props,
    ) -> anyhow::Result<()> {
        if let Some(existing) = self.nodes.get_mut(table).and_then(|t| t.get_mut(node_id)) {
            for (k, v) in properties {
                existing.insert(k.clone(), v.clone());
            }
        }
        Ok(())
    }

    fn delete_node(&mut self, table: &str, node_id: &str) -> anyhow::Result<()> {
        if let Some(t) = self.nodes.get_mut(table) {
            t.remove(node_id);
        }
        self.edges
            .retain(|(_, from, to, _)| from != node_id && to != node_id);
        Ok(())
    }

    fn query_nodes(
        &self,
        table: &str,
        filters: Option<&Props>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>> {
        let Some(table_nodes) = self.nodes.get(table) else {
            return Ok(Vec::new());
        };
        let results: Vec<Props> = table_nodes
            .values()
            .filter(|props| filters.is_none_or(|f| f.iter().all(|(k, v)| props.get(k) == Some(v))))
            .take(limit)
            .cloned()
            .collect();
        Ok(results)
    }

    fn search_nodes(
        &self,
        table: &str,
        text: &str,
        fields: Option<&[&str]>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>> {
        let Some(table_nodes) = self.nodes.get(table) else {
            return Ok(Vec::new());
        };
        let text_lower = text.to_lowercase();
        let results: Vec<Props> = table_nodes
            .values()
            .filter(|props| Self::text_matches(props, &text_lower, fields))
            .take(limit)
            .cloned()
            .collect();
        Ok(results)
    }

    fn create_edge(
        &mut self,
        rel_type: &str,
        _from_table: &str,
        from_id: &str,
        _to_table: &str,
        to_id: &str,
        properties: &Props,
    ) -> anyhow::Result<()> {
        self.edges.push((
            rel_type.to_string(),
            from_id.to_string(),
            to_id.to_string(),
            properties.clone(),
        ));
        Ok(())
    }

    fn get_edges(
        &self,
        node_id: &str,
        rel_type: Option<&str>,
        direction: EdgeDirection,
    ) -> anyhow::Result<Vec<EdgeRecord>> {
        let results = self
            .edges
            .iter()
            .filter(|(rt, from, to, _)| {
                let type_match = rel_type.is_none_or(|t| t == rt);
                let dir_match = match direction {
                    EdgeDirection::Outgoing => from == node_id,
                    EdgeDirection::Incoming => to == node_id,
                    EdgeDirection::Both => from == node_id || to == node_id,
                };
                type_match && dir_match
            })
            .map(|(rt, from, to, props)| EdgeRecord {
                rel_type: rt.clone(),
                from_id: from.clone(),
                to_id: to.clone(),
                properties: props.clone(),
            })
            .collect();
        Ok(results)
    }

    fn delete_edge(&mut self, rel_type: &str, from_id: &str, to_id: &str) -> anyhow::Result<()> {
        self.edges
            .retain(|(rt, f, t, _)| !(rt == rel_type && f == from_id && t == to_id));
        Ok(())
    }

    fn ensure_table(&mut self, table: &str, _schema: &Props) -> anyhow::Result<()> {
        self.nodes.entry(table.to_string()).or_default();
        Ok(())
    }

    fn get_all_node_ids(&self, table: Option<&str>) -> anyhow::Result<HashSet<String>> {
        let mut ids = HashSet::new();
        match table {
            Some(t) => {
                if let Some(nodes) = self.nodes.get(t) {
                    ids.extend(nodes.keys().cloned());
                }
            }
            None => {
                for nodes in self.nodes.values() {
                    ids.extend(nodes.keys().cloned());
                }
            }
        }
        Ok(ids)
    }

    fn export_nodes(&self, node_ids: Option<&[String]>) -> anyhow::Result<Vec<NodeTriple>> {
        let id_set: Option<HashSet<&str>> =
            node_ids.map(|ids| ids.iter().map(|s| s.as_str()).collect());
        let mut result = Vec::new();
        for (table, nodes) in &self.nodes {
            for (id, props) in nodes {
                if id_set.as_ref().is_none_or(|set| set.contains(id.as_str())) {
                    result.push((table.clone(), id.clone(), props.clone()));
                }
            }
        }
        Ok(result)
    }

    fn export_edges(&self, node_ids: Option<&[String]>) -> anyhow::Result<Vec<EdgeQuad>> {
        let id_set: Option<HashSet<&str>> =
            node_ids.map(|ids| ids.iter().map(|s| s.as_str()).collect());
        let result = self
            .edges
            .iter()
            .filter(|(_, from, to, _)| {
                id_set
                    .as_ref()
                    .is_none_or(|set| set.contains(from.as_str()) || set.contains(to.as_str()))
            })
            .map(|(rt, from, to, props)| (rt.clone(), from.clone(), to.clone(), props.clone()))
            .collect();
        Ok(result)
    }

    fn import_nodes(&mut self, nodes: &[NodeTriple]) -> anyhow::Result<usize> {
        let mut count = 0;
        for (table, id, props) in nodes {
            let table_nodes = self.nodes.entry(table.clone()).or_default();
            if !table_nodes.contains_key(id) {
                table_nodes.insert(id.clone(), props.clone());
                count += 1;
            }
        }
        Ok(count)
    }

    fn import_edges(&mut self, edges: &[EdgeQuad]) -> anyhow::Result<usize> {
        let mut existing: HashSet<_> = self
            .edges
            .iter()
            .map(|(rt, f, t, _)| (rt.clone(), f.clone(), t.clone()))
            .collect();
        let mut count = 0;
        for (rt, from, to, props) in edges {
            let key = (rt.clone(), from.clone(), to.clone());
            if !existing.contains(&key) {
                self.edges
                    .push((rt.clone(), from.clone(), to.clone(), props.clone()));
                existing.insert(key);
                count += 1;
            }
        }
        Ok(count)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        self.nodes.clear();
        self.edges.clear();
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/memory_store_tests.rs"]
mod tests;
