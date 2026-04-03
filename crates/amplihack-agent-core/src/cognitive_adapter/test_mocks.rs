//! Shared mock implementations for cognitive adapter tests.

use std::collections::HashMap;

use serde_json::Value;

use super::adapter::CognitiveAdapter;
use super::constants::MAX_WORKING_SLOTS;
use super::types::*;
use crate::agentic_loop::types::MemoryFact;

// ===================================================================
// Mock backend
// ===================================================================
pub(super) struct MockCognitiveBackend {
    kind: BackendKind,
    facts: Vec<MemoryFact>,
    working: HashMap<String, Vec<WorkingSlot>>,
    procedures: Vec<Procedure>,
    prospective: Vec<(String, String, String, String)>,
    episodes: Vec<(String, String)>,
    next_id: usize,
}

impl MockCognitiveBackend {
    pub(super) fn new(kind: BackendKind) -> Self {
        Self {
            kind,
            facts: Vec::new(),
            working: HashMap::new(),
            procedures: Vec::new(),
            prospective: Vec::new(),
            episodes: Vec::new(),
            next_id: 0,
        }
    }

    fn next_id(&mut self) -> String {
        self.next_id += 1;
        format!("id-{}", self.next_id)
    }
}

impl CognitiveBackend for MockCognitiveBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        _source_id: &str,
        _tags: &[String],
        _metadata: &HashMap<String, Value>,
    ) -> String {
        let id = self.next_id();
        self.facts.push(MemoryFact {
            id: id.clone(),
            context: concept.to_string(),
            outcome: content.to_string(),
            confidence,
            metadata: HashMap::new(),
        });
        id
    }

    fn search_facts(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<MemoryFact> {
        let q = query.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.confidence >= min_confidence
                    && (f.outcome.to_lowercase().contains(&q)
                        || f.context.to_lowercase().contains(&q))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact> {
        self.facts.iter().take(limit).cloned().collect()
    }

    fn push_working(
        &mut self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> Option<String> {
        let count = self.working.get(task_id).map_or(0, |v| v.len());
        if count >= MAX_WORKING_SLOTS {
            return None;
        }
        let id = self.next_id();
        self.working.entry(task_id.to_string()).or_default().push(WorkingSlot {
            id: id.clone(),
            slot_type: slot_type.to_string(),
            content: content.to_string(),
            task_id: task_id.to_string(),
            relevance,
        });
        Some(id)
    }

    fn get_working(&self, task_id: &str) -> Vec<WorkingSlot> {
        self.working.get(task_id).cloned().unwrap_or_default()
    }

    fn clear_working(&mut self, task_id: &str) -> usize {
        self.working.remove(task_id).map_or(0, |v| v.len())
    }

    fn store_procedure(&mut self, name: &str, steps: &[String]) -> Option<String> {
        let id = self.next_id();
        self.procedures.push(Procedure {
            id: id.clone(),
            name: name.to_string(),
            steps: steps.to_vec(),
        });
        Some(id)
    }

    fn recall_procedure(&self, query: &str, limit: usize) -> Vec<Procedure> {
        let q = query.to_lowercase();
        self.procedures
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&q))
            .take(limit)
            .cloned()
            .collect()
    }

    fn store_prospective(
        &mut self,
        description: &str,
        trigger_condition: &str,
        action: &str,
    ) -> Option<String> {
        let id = self.next_id();
        self.prospective.push((
            id.clone(),
            description.to_string(),
            trigger_condition.to_string(),
            action.to_string(),
        ));
        Some(id)
    }

    fn check_triggers(&self, content: &str) -> Vec<ProspectiveTrigger> {
        let c = content.to_lowercase();
        self.prospective
            .iter()
            .filter(|(_, _, trigger, _)| c.contains(&trigger.to_lowercase()))
            .map(|(id, desc, trigger, action)| ProspectiveTrigger {
                id: id.clone(),
                description: desc.clone(),
                trigger_condition: trigger.clone(),
                action: action.clone(),
            })
            .collect()
    }

    fn record_sensory(
        &mut self,
        _modality: &str,
        _raw_data: &str,
        _ttl_seconds: u64,
    ) -> Option<String> {
        Some(self.next_id())
    }

    fn store_episode(&mut self, content: &str, source_label: &str) -> String {
        let id = self.next_id();
        self.episodes
            .push((content.to_string(), source_label.to_string()));
        id
    }

    fn get_statistics(&self) -> HashMap<String, Value> {
        let mut stats = HashMap::new();
        stats.insert(
            "total".into(),
            serde_json::to_value(self.facts.len()).unwrap(),
        );
        stats
    }
}

// ===================================================================
// Mock hive store
// ===================================================================

pub(super) struct MockHiveStore {
    facts: Vec<HiveFact>,
    agents: Vec<String>,
}

impl MockHiveStore {
    pub(super) fn new() -> Self {
        Self {
            facts: Vec::new(),
            agents: Vec::new(),
        }
    }

    pub(super) fn with_facts(facts: Vec<HiveFact>) -> Self {
        Self {
            facts,
            agents: Vec::new(),
        }
    }
}

impl HiveStore for MockHiveStore {
    fn promote_fact(&mut self, _agent_name: &str, fact: &HiveFact) -> Result<(), String> {
        self.facts.push(fact.clone());
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Vec<HiveFact> {
        let q = query.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.content.to_lowercase().contains(&q)
                    || f.concept.to_lowercase().contains(&q)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn get_all_facts(&self, limit: usize) -> Vec<HiveFact> {
        self.facts.iter().take(limit).cloned().collect()
    }

    fn register_agent(&mut self, agent_name: &str) {
        self.agents.push(agent_name.to_string());
    }
}

// ===================================================================
// Mock quality scorer
// ===================================================================

pub(super) struct AlwaysPassScorer;
impl QualityScorer for AlwaysPassScorer {
    fn score(&self, _content: &str, _context: &str) -> f64 {
        1.0
    }
}

pub(super) struct AlwaysFailScorer;
impl QualityScorer for AlwaysFailScorer {
    fn score(&self, _content: &str, _context: &str) -> f64 {
        0.0
    }
}

// ===================================================================
// Helper
// ===================================================================

pub(super) fn make_adapter(kind: BackendKind) -> CognitiveAdapter {
    let cfg = CognitiveAdapterConfig::new("test-agent");
    CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(kind)),
        None,
        None,
    )
}

pub(super) fn make_adapter_with_hive(hive: MockHiveStore) -> CognitiveAdapter {
    let cfg = CognitiveAdapterConfig::new("test-agent");
    CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(BackendKind::Cognitive)),
        Some(Box::new(hive)),
        None,
    )
}
