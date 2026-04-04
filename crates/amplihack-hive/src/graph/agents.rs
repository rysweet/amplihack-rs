//! Agent registry for the hive knowledge graph.

use crate::error::HiveError;
use crate::models::HiveAgent;

use super::HiveGraph;

impl HiveGraph {
    pub fn register_agent(&mut self, agent_id: &str, domain: &str) -> crate::error::Result<()> {
        if self.agents.contains_key(agent_id) {
            return Err(HiveError::Graph(format!(
                "agent already registered: {agent_id}"
            )));
        }
        self.agents
            .insert(agent_id.to_string(), HiveAgent::new(agent_id, domain));
        Ok(())
    }

    pub fn unregister_agent(&mut self, agent_id: &str) -> bool {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = "removed".to_string();
            true
        } else {
            false
        }
    }

    pub fn get_agent(&self, agent_id: &str) -> Option<&HiveAgent> {
        self.agents.get(agent_id)
    }

    pub fn list_agents(&self, status_filter: Option<&str>) -> Vec<&HiveAgent> {
        self.agents
            .values()
            .filter(|a| status_filter.is_none_or(|s| a.status == s))
            .collect()
    }

    pub fn update_trust(&mut self, agent_id: &str, trust: f64) -> bool {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.trust = trust.clamp(0.0, 2.0);
            true
        } else {
            false
        }
    }
}
