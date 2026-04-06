//! [`HiveCoordinator`] — lightweight in-memory expertise registry and trust tracker.

use super::MAX_CONTRADICTIONS;
use crate::models::{DEFAULT_TRUST_SCORE, MAX_TRUST_SCORE};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
struct AgentRecord {
    domain: String,
    fact_count: u64,
    topics: HashSet<String>,
}

/// A detected contradiction between two facts.
#[derive(Clone, Debug)]
pub(crate) struct Contradiction {
    pub _fact_a: String,
    pub _fact_b: String,
    pub resolved: bool,
}

/// Lightweight in-memory registry tracking agent expertise, trust, and contradictions.
pub struct HiveCoordinator {
    agents: HashMap<String, AgentRecord>,
    expertise: HashMap<String, HashSet<String>>,
    trust: HashMap<String, f64>,
    contradictions: Vec<Contradiction>,
}

impl HiveCoordinator {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            expertise: HashMap::new(),
            trust: HashMap::new(),
            contradictions: Vec::new(),
        }
    }

    pub fn register_agent(&mut self, agent_id: &str, domain: &str) {
        let keywords: HashSet<String> = domain
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 1)
            .collect();
        for kw in &keywords {
            self.expertise
                .entry(kw.clone())
                .or_default()
                .insert(agent_id.to_string());
        }
        self.agents.insert(
            agent_id.to_string(),
            AgentRecord {
                domain: domain.to_string(),
                fact_count: 0,
                topics: HashSet::new(),
            },
        );
        self.trust
            .entry(agent_id.to_string())
            .or_insert(DEFAULT_TRUST_SCORE);
    }

    pub fn unregister_agent(&mut self, agent_id: &str) {
        self.agents.remove(agent_id);
        self.trust.remove(agent_id);
        for agents in self.expertise.values_mut() {
            agents.remove(agent_id);
        }
    }

    pub fn get_experts(&self, topic: &str) -> Vec<String> {
        let kw = topic.to_lowercase();
        let mut experts: Vec<String> = self
            .expertise
            .get(&kw)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        experts.sort_by(|a, b| {
            let ta = self.trust.get(a).copied().unwrap_or(DEFAULT_TRUST_SCORE);
            let tb = self.trust.get(b).copied().unwrap_or(DEFAULT_TRUST_SCORE);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });
        experts
    }

    pub fn route_query(&self, query: &str) -> Vec<String> {
        let keywords: Vec<String> = query
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 1)
            .collect();
        let mut scores: HashMap<String, f64> = HashMap::new();
        for kw in &keywords {
            if let Some(agents) = self.expertise.get(kw) {
                for agent_id in agents {
                    *scores.entry(agent_id.clone()).or_default() += 1.0;
                }
            }
        }
        for (agent_id, record) in &self.agents {
            for kw in &keywords {
                if record.topics.contains(kw) || record.domain.to_lowercase().contains(kw.as_str())
                {
                    *scores.entry(agent_id.clone()).or_default() += 1.0;
                }
            }
        }
        for (agent_id, score) in scores.iter_mut() {
            *score *= self
                .trust
                .get(agent_id)
                .copied()
                .unwrap_or(DEFAULT_TRUST_SCORE);
        }
        let mut ranked: Vec<(String, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.into_iter().map(|(id, _)| id).collect()
    }

    pub fn report_fact(&mut self, agent_id: &str, concept: &str) {
        if let Some(record) = self.agents.get_mut(agent_id) {
            record.fact_count += 1;
            for kw in concept.split_whitespace().map(|w| w.to_lowercase()) {
                if kw.len() > 1 {
                    record.topics.insert(kw.clone());
                    self.expertise
                        .entry(kw)
                        .or_default()
                        .insert(agent_id.to_string());
                }
            }
        }
    }

    pub fn check_trust(&self, agent_id: &str) -> f64 {
        self.trust
            .get(agent_id)
            .copied()
            .unwrap_or(DEFAULT_TRUST_SCORE)
    }

    pub fn update_trust(&mut self, agent_id: &str, delta: f64) {
        let entry = self
            .trust
            .entry(agent_id.to_string())
            .or_insert(DEFAULT_TRUST_SCORE);
        *entry = (*entry + delta).clamp(0.0, MAX_TRUST_SCORE);
    }

    pub fn report_contradiction(&mut self, fact_a: &str, fact_b: &str) {
        if self.contradictions.len() >= MAX_CONTRADICTIONS {
            self.contradictions.remove(0);
        }
        self.contradictions.push(Contradiction {
            _fact_a: fact_a.to_string(),
            _fact_b: fact_b.to_string(),
            resolved: false,
        });
    }

    pub fn resolve_contradiction(&mut self, index: usize) -> bool {
        if let Some(c) = self.contradictions.get_mut(index) {
            c.resolved = true;
            true
        } else {
            false
        }
    }

    pub fn get_hive_stats(&self) -> serde_json::Value {
        let agents: serde_json::Value = self
            .agents
            .iter()
            .map(|(id, r)| {
                (
                    id.clone(),
                    serde_json::json!({
                        "domain": r.domain, "fact_count": r.fact_count,
                        "trust": self.trust.get(id).copied().unwrap_or(DEFAULT_TRUST_SCORE),
                        "topic_count": r.topics.len(),
                    }),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into();
        serde_json::json!({
            "agent_count": self.agents.len(), "agents": agents,
            "expertise_keywords": self.expertise.len(),
            "contradictions": self.contradictions.len(),
            "unresolved_contradictions": self.contradictions.iter().filter(|c| !c.resolved).count(),
        })
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }
}

impl Default for HiveCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_unregister() {
        let mut c = HiveCoordinator::new();
        c.register_agent("a1", "security research");
        assert_eq!(c.agent_count(), 1);
        c.unregister_agent("a1");
        assert_eq!(c.agent_count(), 0);
    }

    #[test]
    fn get_experts_by_keyword() {
        let mut c = HiveCoordinator::new();
        c.register_agent("a1", "security");
        c.register_agent("a2", "networking");
        assert_eq!(c.get_experts("security"), vec!["a1"]);
    }

    #[test]
    fn route_query_ranks_by_trust() {
        let mut c = HiveCoordinator::new();
        c.register_agent("a1", "security");
        c.register_agent("a2", "security");
        c.update_trust("a2", 0.5);
        assert_eq!(c.route_query("security")[0], "a2");
    }

    #[test]
    fn report_fact_updates_topics() {
        let mut c = HiveCoordinator::new();
        c.register_agent("a1", "research");
        c.report_fact("a1", "rust memory safety");
        assert!(c.get_experts("rust").contains(&"a1".to_string()));
    }

    #[test]
    fn trust_clamped() {
        let mut c = HiveCoordinator::new();
        c.register_agent("a1", "d");
        c.update_trust("a1", 100.0);
        assert!((c.check_trust("a1") - MAX_TRUST_SCORE).abs() < f64::EPSILON);
        c.update_trust("a1", -100.0);
        assert!(c.check_trust("a1").abs() < f64::EPSILON);
    }

    #[test]
    fn contradictions_capped() {
        let mut c = HiveCoordinator::new();
        for i in 0..MAX_CONTRADICTIONS + 5 {
            c.report_contradiction(&format!("a-{i}"), &format!("b-{i}"));
        }
        assert_eq!(c.contradictions.len(), MAX_CONTRADICTIONS);
    }

    #[test]
    fn resolve_contradiction() {
        let mut c = HiveCoordinator::new();
        c.report_contradiction("fa", "fb");
        assert!(c.resolve_contradiction(0));
        assert!(!c.resolve_contradiction(99));
    }

    #[test]
    fn hive_stats() {
        let mut c = HiveCoordinator::new();
        c.register_agent("a1", "security");
        c.report_fact("a1", "vuln");
        assert_eq!(c.get_hive_stats()["agent_count"], 1);
    }
}
