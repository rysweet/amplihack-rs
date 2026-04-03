use std::collections::HashMap;

use crate::error::{HiveError, Result};
use crate::event_bus::{EventBus, LocalEventBus};
use crate::models::{
    make_event, HiveFact, HiveManifest, HiveState, DEFAULT_CONTRADICTION_OVERLAP,
};
use crate::orchestrator::{HiveMindOrchestrator, PromotionResult};

/// Dict-based fact store for lightweight use.
pub struct InMemoryGraphStore {
    facts: HashMap<String, HiveFact>,
}

impl InMemoryGraphStore {
    pub fn new() -> Self { Self { facts: HashMap::new() } }
    pub fn insert(&mut self, fact: HiveFact) { self.facts.insert(fact.fact_id.clone(), fact); }
    pub fn get(&self, fact_id: &str) -> Option<&HiveFact> { self.facts.get(fact_id) }

    pub fn query(&self, concept: &str, min_confidence: f64) -> Vec<&HiveFact> {
        let c = concept.to_lowercase();
        self.facts.values()
            .filter(|f| f.concept.to_lowercase().contains(&c) && f.confidence >= min_confidence)
            .collect()
    }

    pub fn len(&self) -> usize { self.facts.len() }
    pub fn is_empty(&self) -> bool { self.facts.is_empty() }
}

impl Default for InMemoryGraphStore { fn default() -> Self { Self::new() } }

/// Trust checks + contradiction detection with word overlap.
pub struct InMemoryGateway {
    trust_threshold: f64,
    contradiction_overlap: f64,
}

impl InMemoryGateway {
    pub fn new(trust_threshold: f64, contradiction_overlap: f64) -> Self {
        Self { trust_threshold, contradiction_overlap }
    }

    pub fn passes_trust(&self, confidence: f64) -> bool {
        confidence >= self.trust_threshold
    }

    pub fn is_contradiction(&self, a: &str, b: &str) -> bool {
        let wa: std::collections::HashSet<&str> = a.split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric())).collect();
        let wb: std::collections::HashSet<&str> = b.split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric())).collect();
        if wa.is_empty() || wb.is_empty() { return false; }
        let overlap = wa.intersection(&wb).count();
        let min_len = wa.len().min(wb.len());
        if min_len == 0 { return false; }
        (overlap as f64 / min_len as f64) >= self.contradiction_overlap
    }
}

impl Default for InMemoryGateway {
    fn default() -> Self { Self::new(0.0, DEFAULT_CONTRADICTION_OVERLAP) }
}

/// Kubernetes-style reconciliation controller for the hive.
pub struct HiveController {
    desired: Option<HiveManifest>,
    current: HiveState,
    orchestrators: HashMap<String, HiveMindOrchestrator>,
    bus: LocalEventBus,
    gateway: InMemoryGateway,
}

impl HiveController {
    pub fn new() -> Self {
        Self {
            desired: None,
            current: HiveState {
                running_agents: vec![], graph_status: "idle".into(), bus_status: "idle".into(),
                agents: HashMap::new(), hive_store_connected: false, event_bus_connected: false,
            },
            orchestrators: HashMap::new(), bus: LocalEventBus::new(),
            gateway: InMemoryGateway::default(),
        }
    }

    pub fn from_manifest(manifest: HiveManifest) -> Self {
        let gw = InMemoryGateway::new(
            manifest.gateway.trust_threshold, manifest.gateway.contradiction_overlap,
        );
        Self {
            desired: Some(manifest),
            current: HiveState {
                running_agents: vec![], graph_status: "idle".into(), bus_status: "idle".into(),
                agents: HashMap::new(), hive_store_connected: true, event_bus_connected: true,
            },
            orchestrators: HashMap::new(), bus: LocalEventBus::new(), gateway: gw,
        }
    }

    pub fn apply_manifest(&mut self, manifest: HiveManifest) -> Result<Vec<String>> {
        let desired_names: std::collections::HashSet<String> =
            manifest.agents.iter().map(|a| a.name.clone()).collect();
        let current_names: std::collections::HashSet<String> =
            self.orchestrators.keys().cloned().collect();
        let mut actions = Vec::new();
        for agent in &manifest.agents {
            if !current_names.contains(&agent.name) {
                let orch = HiveMindOrchestrator::with_default_policy()
                    .with_agent_id(agent.name.clone());
                self.orchestrators.insert(agent.name.clone(), orch);
                self.bus.subscribe(&agent.name, None)?;
                actions.push(format!("create {}", agent.name));
            }
        }
        let to_remove: Vec<String> = current_names.difference(&desired_names).cloned().collect();
        for name in &to_remove {
            self.orchestrators.remove(name);
            self.bus.unsubscribe(name)?;
            actions.push(format!("remove {}", name));
        }
        self.desired = Some(manifest.clone());
        self.current.running_agents = manifest.agents.clone();
        self.current.hive_store_connected = true;
        self.current.event_bus_connected = true;
        Ok(actions)
    }

    pub fn learn(
        &mut self, agent_name: &str, concept: &str, content: &str, confidence: f64,
    ) -> Result<PromotionResult> {
        let orch = self.orchestrators.get_mut(agent_name)
            .ok_or_else(|| HiveError::Controller(format!("agent not found: {agent_name}")))?;
        orch.store_and_promote(concept, content, confidence, agent_name)
    }

    pub fn promote_fact(&mut self, agent_name: &str, fact_id: &str) -> Result<bool> {
        let orch = self.orchestrators.get_mut(agent_name)
            .ok_or_else(|| HiveError::Controller(format!("agent not found: {agent_name}")))?;
        orch.promote(fact_id, agent_name)
    }

    pub fn query_agent(&self, agent_name: &str, concept: &str) -> Result<Vec<HiveFact>> {
        let orch = self.orchestrators.get(agent_name)
            .ok_or_else(|| HiveError::Controller(format!("agent not found: {agent_name}")))?;
        orch.query(concept)
    }

    pub fn query_routed(&self, concept: &str, limit: usize) -> Result<Vec<HiveFact>> {
        let mut all = Vec::new();
        for orch in self.orchestrators.values() { all.extend(orch.query(concept)?); }
        all.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        let mut seen = std::collections::HashSet::new();
        all.retain(|f| seen.insert(f.content.clone()));
        all.truncate(limit);
        Ok(all)
    }

    pub fn propagate(
        &mut self, source_agent: &str, concept: &str, content: &str, confidence: f64,
    ) -> Result<()> {
        if !self.gateway.passes_trust(confidence) { return Ok(()); }
        let ev = make_event("fact.propagate", source_agent,
            serde_json::json!({"concept": concept, "content": content, "confidence": confidence}));
        self.bus.publish(ev)?;
        let names: Vec<String> = self.orchestrators.keys().cloned().collect();
        for name in names {
            let events = self.bus.poll(&name)?;
            for event in &events {
                if let Some(orch) = self.orchestrators.get_mut(&name) {
                    let _ = orch.process_event(event);
                }
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        for orch in self.orchestrators.values_mut() { let _ = orch.close(); }
        self.orchestrators.clear();
        self.bus.close()?;
        self.current.running_agents.clear();
        self.current.graph_status = "stopped".into();
        self.current.bus_status = "stopped".into();
        self.current.hive_store_connected = false;
        self.current.event_bus_connected = false;
        Ok(())
    }

    pub fn gateway(&self) -> &InMemoryGateway { &self.gateway }

    pub fn apply(&mut self, manifest: HiveManifest) -> Result<()> {
        self.desired = Some(manifest);
        Ok(())
    }

    pub fn reconcile(&mut self) -> Result<Vec<String>> {
        let Some(desired) = &self.desired else { return Ok(vec![]); };
        let mut actions = Vec::new();
        let current_map: std::collections::HashMap<&str, u32> = self.current.running_agents
            .iter().map(|a| (a.name.as_str(), a.replicas)).collect();
        let desired_names: std::collections::HashSet<&str> =
            desired.agents.iter().map(|a| a.name.as_str()).collect();
        for agent in &desired.agents {
            match current_map.get(agent.name.as_str()) {
                Some(&r) if r != agent.replicas => {
                    actions.push(format!("scale {} to {}", agent.name, agent.replicas));
                }
                None => actions.push(format!("scale {} to {}", agent.name, agent.replicas)),
                _ => {}
            }
        }
        for ca in &self.current.running_agents {
            if !desired_names.contains(ca.name.as_str()) {
                actions.push(format!("remove {}", ca.name));
            }
        }
        self.current.running_agents = desired.agents.clone();
        Ok(actions)
    }

    pub fn status(&self) -> &HiveState { &self.current }
    pub fn desired_manifest(&self) -> Option<&HiveManifest> { self.desired.as_ref() }

    pub fn scale_agent(&mut self, name: &str, replicas: u32) -> Result<()> {
        let mut found = false;
        if let Some(manifest) = &mut self.desired
            && let Some(agent) = manifest.agents.iter_mut().find(|a| a.name == name)
        { agent.replicas = replicas; found = true; }
        if let Some(current) = self.current.running_agents.iter_mut().find(|a| a.name == name)
        { current.replicas = replicas; found = true; }
        if found { Ok(()) } else { Err(HiveError::Controller(format!("agent not found: {name}"))) }
    }

    pub fn remove_agent(&mut self, name: &str) -> Result<bool> {
        let mut found = false;
        if let Some(manifest) = &mut self.desired {
            let before = manifest.agents.len();
            manifest.agents.retain(|a| a.name != name);
            if manifest.agents.len() < before { found = true; }
        }
        let before = self.current.running_agents.len();
        self.current.running_agents.retain(|a| a.name != name);
        if self.current.running_agents.len() < before { found = true; }
        Ok(found)
    }
}

impl Default for HiveController { fn default() -> Self { Self::new() } }
