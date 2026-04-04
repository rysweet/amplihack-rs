//! [`DistributedHiveGraph`] — DHT-sharded knowledge graph with gossip convergence.

use super::POSITION_SCORE_DECREMENT;
use super::merge::merge_ranked_shard_results;
use super::transport::{LocalShardTransport, ShardTransport};
use crate::bloom::BloomFilter;
use crate::dht::{DHTRouter, ShardFact};
use crate::models::{
    DEFAULT_BROADCAST_THRESHOLD, HiveAgent, HiveEdge, HiveFact, PEER_CONFIDENCE_DISCOUNT,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// DHT-sharded distributed knowledge graph.
pub struct DistributedHiveGraph {
    hive_id: String,
    router: Arc<Mutex<DHTRouter>>,
    agents: HashMap<String, HiveAgent>,
    edges: HashMap<String, Vec<HiveEdge>>,
    bloom_filters: HashMap<String, BloomFilter>,
    enable_gossip: bool,
    broadcast_threshold: f64,
    parent: Option<Box<DistributedHiveGraph>>,
    children: Vec<DistributedHiveGraph>,
    total_promotes: u64,
    transport: Box<dyn ShardTransport>,
}

impl DistributedHiveGraph {
    pub fn new() -> Self {
        Self::with_config(3, 5, true, DEFAULT_BROADCAST_THRESHOLD)
    }

    pub fn with_config(replication: usize, fanout: usize, gossip: bool, bcast: f64) -> Self {
        let router = Arc::new(Mutex::new(DHTRouter::new(replication, fanout)));
        let transport = Box::new(LocalShardTransport::new(Arc::clone(&router)));
        Self {
            hive_id: Uuid::new_v4().to_string(),
            router,
            agents: HashMap::new(),
            edges: HashMap::new(),
            bloom_filters: HashMap::new(),
            enable_gossip: gossip,
            broadcast_threshold: bcast,
            parent: None,
            children: Vec::new(),
            total_promotes: 0,
            transport,
        }
    }

    pub fn hive_id(&self) -> &str {
        &self.hive_id
    }

    pub fn register_agent(&mut self, agent_id: &str, domain: &str, trust: f64) {
        if let Ok(mut r) = self.router.lock() {
            r.add_agent(agent_id);
        }
        self.agents.insert(
            agent_id.to_string(),
            HiveAgent {
                agent_id: agent_id.to_string(),
                domain: domain.to_string(),
                trust,
                fact_count: 0,
                status: "active".to_string(),
            },
        );
        self.bloom_filters
            .insert(agent_id.to_string(), BloomFilter::new(500, 0.01));
    }

    pub fn unregister_agent(&mut self, agent_id: &str) {
        let orphans = if let Ok(mut r) = self.router.lock() {
            r.remove_agent(agent_id)
        } else {
            Vec::new()
        };
        self.agents.remove(agent_id);
        self.bloom_filters.remove(agent_id);
        for fact in orphans {
            if let Ok(mut r) = self.router.lock() {
                r.store_fact(fact);
            }
        }
    }

    pub fn get_agent(&self, agent_id: &str) -> Option<&HiveAgent> {
        self.agents.get(agent_id)
    }
    pub fn list_agents(&self) -> Vec<&HiveAgent> {
        self.agents.values().collect()
    }

    pub fn update_trust(&mut self, agent_id: &str, trust: f64) {
        if let Some(a) = self.agents.get_mut(agent_id) {
            a.trust = trust;
        }
    }

    pub fn promote_fact(&mut self, agent_id: &str, fact: HiveFact) -> String {
        let fact_id = fact.fact_id.clone();
        let shard_fact = hive_fact_to_shard(&fact);
        self.transport.store_on_shard(agent_id, shard_fact);
        if let Some(bloom) = self.bloom_filters.get_mut(agent_id) {
            bloom.add(&fact_id);
        }
        if let Some(a) = self.agents.get_mut(agent_id) {
            a.fact_count += 1;
        }
        self.total_promotes += 1;
        if fact.confidence >= self.broadcast_threshold {
            self.escalate_fact(&fact);
        }
        fact_id
    }

    pub fn get_fact(&self, fact_id: &str) -> Option<ShardFact> {
        if let Ok(r) = self.router.lock() {
            for aid in r.all_agents() {
                if let Some(shard) = r.get_shard(&aid)
                    && let Some(f) = shard.get(fact_id)
                {
                    return Some(f.clone());
                }
            }
        }
        None
    }

    pub fn retract_fact(&self, fact_id: &str) -> bool {
        if let Ok(r) = self.router.lock() {
            for aid in r.all_agents() {
                if let Some(shard) = r.get_shard(&aid)
                    && shard.get(fact_id).is_some()
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn query_facts(&self, query: &str, limit: usize) -> Vec<ShardFact> {
        let targets = if let Ok(r) = self.router.lock() {
            r.select_query_targets(query)
        } else {
            return Vec::new();
        };
        let mut results_by_agent: Vec<(String, Vec<ShardFact>)> = Vec::new();
        for aid in &targets {
            let facts = self.transport.query_shard(aid, query, limit);
            if !facts.is_empty() {
                results_by_agent.push((aid.clone(), facts));
            }
        }
        merge_ranked_shard_results(&results_by_agent, limit, POSITION_SCORE_DECREMENT)
    }

    pub fn add_edge(&mut self, edge: HiveEdge) {
        self.edges
            .entry(edge.source_id.clone())
            .or_default()
            .push(edge);
    }

    pub fn get_edges(&self, node_id: &str, edge_type: Option<&str>) -> Vec<&HiveEdge> {
        self.edges
            .get(node_id)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|e| edge_type.is_none_or(|t| e.edge_type == t))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn route_query(&self, query: &str) -> Vec<String> {
        let kws: HashSet<String> = query
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 1)
            .collect();
        let mut scored: Vec<(String, usize)> = self
            .agents
            .iter()
            .filter_map(|(id, a)| {
                let dw: HashSet<String> = a
                    .domain
                    .split_whitespace()
                    .map(|w| w.to_lowercase())
                    .collect();
                let overlap = kws.intersection(&dw).count();
                if overlap > 0 {
                    Some((id.clone(), overlap))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(id, _)| id).collect()
    }

    pub fn set_parent(&mut self, parent: DistributedHiveGraph) {
        self.parent = Some(Box::new(parent));
    }
    pub fn add_child(&mut self, child: DistributedHiveGraph) {
        self.children.push(child);
    }

    fn escalate_fact(&mut self, fact: &HiveFact) {
        if let Some(parent) = &mut self.parent {
            let mut esc = fact.clone();
            esc.tags.push(format!("escalation:{}", self.hive_id));
            parent.promote_fact(&fact.source_id, esc);
        }
    }

    pub fn query_federated(
        &self,
        query: &str,
        limit: usize,
        visited: &mut HashSet<String>,
    ) -> Vec<ShardFact> {
        if !visited.insert(self.hive_id.clone()) {
            return Vec::new();
        }
        let mut local = self.query_facts(query, limit);
        for child in &self.children {
            local.extend(child.query_federated(query, limit, visited));
        }
        let mut seen = HashSet::new();
        local.retain(|f| seen.insert(f.content.clone()));
        local.truncate(limit);
        local
    }

    pub fn run_gossip_round(&mut self) -> HashMap<String, usize> {
        if !self.enable_gossip {
            return HashMap::new();
        }
        let aids: Vec<String> = self.agents.keys().cloned().collect();
        let mut pulled = HashMap::new();
        for aid in &aids {
            let peers = select_gossip_peers(aid, &aids, 2);
            let mut count = 0usize;
            for pid in &peers {
                count += self.pull_missing_facts(aid, pid);
            }
            pulled.insert(aid.clone(), count);
        }
        pulled
    }

    fn pull_missing_facts(&mut self, agent_id: &str, peer_id: &str) -> usize {
        let (my_bloom, peer_ids) = {
            let r = match self.router.lock() {
                Ok(r) => r,
                Err(_) => return 0,
            };
            let bloom = self.bloom_filters.get(agent_id).cloned();
            let ids: Vec<String> = r
                .get_shard(peer_id)
                .map(|s| s.fact_ids().into_iter().collect())
                .unwrap_or_default();
            (bloom, ids)
        };
        let bloom = match my_bloom {
            Some(b) => b,
            None => return 0,
        };
        let refs: Vec<&str> = peer_ids.iter().map(|s| s.as_str()).collect();
        let missing = bloom.missing_from(&refs);
        let facts: Vec<ShardFact> = {
            let r = match self.router.lock() {
                Ok(r) => r,
                Err(_) => return 0,
            };
            missing
                .iter()
                .filter_map(|fid| r.get_shard(peer_id).and_then(|s| s.get(fid)).cloned())
                .collect()
        };
        let mut count = 0;
        for mut fact in facts {
            fact.confidence *= PEER_CONFIDENCE_DISCOUNT;
            fact.tags.push(format!("gossip_from:{peer_id}"));
            self.transport.store_on_shard(agent_id, fact.clone());
            if let Some(bloom) = self.bloom_filters.get_mut(agent_id) {
                bloom.add(&fact.fact_id);
            }
            count += 1;
        }
        count
    }

    pub fn convergence_score(&self) -> f64 {
        let aids: Vec<&String> = self.agents.keys().collect();
        if aids.len() < 2 {
            return 1.0;
        }
        let all_ids: HashSet<String> = if let Ok(r) = self.router.lock() {
            aids.iter()
                .filter_map(|a| r.get_shard(a))
                .flat_map(|s| s.fact_ids())
                .collect()
        } else {
            return 0.0;
        };
        if all_ids.is_empty() {
            return 1.0;
        }
        let on_all = all_ids
            .iter()
            .filter(|fid| {
                if let Ok(r) = self.router.lock() {
                    aids.iter()
                        .all(|a| r.get_shard(a).is_some_and(|s| s.get(fid).is_some()))
                } else {
                    false
                }
            })
            .count();
        on_all as f64 / all_ids.len() as f64
    }

    pub fn get_stats(&self) -> serde_json::Value {
        let dht = self.router.lock().map(|r| r.stats()).ok();
        serde_json::json!({
            "hive_id": self.hive_id, "agent_count": self.agents.len(),
            "total_promotes": self.total_promotes,
            "edge_count": self.edges.values().map(|v| v.len()).sum::<usize>(),
            "dht": dht.map(|s| serde_json::json!({
                "total_facts": s.total_facts, "agent_count": s.agent_count,
                "avg_shard_size": s.avg_shard_size,
            })),
        })
    }

    pub fn close(&mut self) {
        self.agents.clear();
        self.edges.clear();
        self.bloom_filters.clear();
    }
}

impl Default for DistributedHiveGraph {
    fn default() -> Self {
        Self::new()
    }
}

fn hive_fact_to_shard(fact: &HiveFact) -> ShardFact {
    ShardFact {
        fact_id: fact.fact_id.clone(),
        content: fact.content.clone(),
        concept: fact.concept.clone(),
        confidence: fact.confidence,
        source_agent: fact.source_id.clone(),
        tags: fact.tags.clone(),
        created_at: fact.created_at.timestamp() as f64,
        metadata: HashMap::new(),
        ring_position: 0,
    }
}

fn select_gossip_peers(agent_id: &str, all: &[String], fanout: usize) -> Vec<String> {
    let peers: Vec<&String> = all.iter().filter(|a| a.as_str() != agent_id).collect();
    if peers.len() <= fanout {
        return peers.into_iter().cloned().collect();
    }
    let hash = crate::dht::hash_key(agent_id) as usize;
    (0..fanout)
        .map(|i| peers[(hash + i) % peers.len()].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DEFAULT_TRUST_SCORE;
    use chrono::Utc;

    fn make_fact(id: &str, content: &str, conf: f64) -> HiveFact {
        HiveFact {
            fact_id: id.to_string(),
            concept: "test".into(),
            content: content.to_string(),
            confidence: conf,
            source_id: "agent-1".into(),
            tags: Vec::new(),
            created_at: Utc::now(),
            status: "promoted".into(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn register_and_promote() {
        let mut g = DistributedHiveGraph::new();
        g.register_agent("a1", "security", DEFAULT_TRUST_SCORE);
        assert_eq!(
            g.promote_fact("a1", make_fact("f1", "SQL injection risk", 0.8)),
            "f1"
        );
        assert_eq!(g.get_agent("a1").unwrap().fact_count, 1);
    }

    #[test]
    fn query_facts_returns_results() {
        let mut g = DistributedHiveGraph::new();
        g.register_agent("a1", "d", DEFAULT_TRUST_SCORE);
        g.register_agent("a2", "d", DEFAULT_TRUST_SCORE);
        g.promote_fact("a1", make_fact("f1", "Rust memory safety model", 0.9));
        assert!(!g.query_facts("Rust memory", 10).is_empty());
    }

    #[test]
    fn edges() {
        let mut g = DistributedHiveGraph::new();
        g.add_edge(HiveEdge {
            source_id: "n1".into(),
            target_id: "n2".into(),
            edge_type: "depends_on".into(),
            properties: HashMap::new(),
        });
        assert_eq!(g.get_edges("n1", None).len(), 1);
        assert!(g.get_edges("n1", Some("other")).is_empty());
    }

    #[test]
    fn route_query_matches_domain() {
        let mut g = DistributedHiveGraph::new();
        g.register_agent("a1", "security research", DEFAULT_TRUST_SCORE);
        g.register_agent("a2", "networking", DEFAULT_TRUST_SCORE);
        assert_eq!(g.route_query("security"), vec!["a1"]);
    }

    #[test]
    fn gossip_round() {
        let mut g = DistributedHiveGraph::with_config(1, 5, true, 0.9);
        g.register_agent("a1", "d", DEFAULT_TRUST_SCORE);
        g.register_agent("a2", "d", DEFAULT_TRUST_SCORE);
        g.promote_fact("a1", make_fact("f1", "shared knowledge item", 0.8));
        let pulled = g.run_gossip_round();
        assert!(pulled.contains_key("a2"));
    }

    #[test]
    fn federated_query() {
        let mut parent = DistributedHiveGraph::new();
        parent.register_agent("pa", "d", DEFAULT_TRUST_SCORE);
        parent.promote_fact("pa", make_fact("pf1", "parent fact about security", 0.9));
        let mut child = DistributedHiveGraph::new();
        child.register_agent("ca", "d", DEFAULT_TRUST_SCORE);
        child.promote_fact("ca", make_fact("cf1", "child fact about security", 0.8));
        parent.add_child(child);
        let mut visited = HashSet::new();
        assert!(
            !parent
                .query_federated("security", 10, &mut visited)
                .is_empty()
        );
    }

    #[test]
    fn convergence_score_single_agent() {
        let mut g = DistributedHiveGraph::new();
        g.register_agent("a1", "d", DEFAULT_TRUST_SCORE);
        assert!((g.convergence_score() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn select_gossip_peers_excludes_self() {
        let agents = vec!["a1".into(), "a2".into(), "a3".into()];
        let peers = select_gossip_peers("a1", &agents, 2);
        assert!(!peers.contains(&"a1".to_string()));
        assert!(peers.len() <= 2);
    }
}
