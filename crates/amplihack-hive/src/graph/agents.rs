use crate::error::Result;
use crate::models::HiveAgent;

use super::HiveGraph;

impl HiveGraph {
    /// Register a new agent in the graph.
    pub fn register_agent(
        &mut self,
        agent_id: impl Into<String>,
        domain: impl Into<String>,
    ) -> Result<()> {
        let agent = HiveAgent::new(agent_id, domain);
        self.agents.insert(agent.agent_id.clone(), agent);
        Ok(())
    }

    /// Unregister an agent by setting its status to "removed".
    /// Returns `true` if the agent was found.
    pub fn unregister_agent(&mut self, agent_id: &str) -> bool {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = "removed".to_string();
            true
        } else {
            false
        }
    }

    /// Get a reference to an agent by ID.
    pub fn get_agent(&self, agent_id: &str) -> Option<&HiveAgent> {
        self.agents.get(agent_id)
    }

    /// List agents, optionally filtered by status.
    pub fn list_agents(&self, status_filter: Option<&str>) -> Vec<&HiveAgent> {
        self.agents
            .values()
            .filter(|a| match status_filter {
                Some(s) => a.status == s,
                None => true,
            })
            .collect()
    }

    /// Update an agent's trust score, clamped to [0.0, 2.0].
    /// Returns `true` if the agent was found.
    pub fn update_trust(&mut self, agent_id: &str, trust: f64) -> bool {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.trust = trust.clamp(0.0, 2.0);
            true
        } else {
            false
        }
    }
}
