use crate::error::{HiveError, Result};
use crate::models::{HiveManifest, HiveState};

/// Kubernetes-style reconciliation controller for the hive.
///
/// Tracks a desired [`HiveManifest`] and reconciles it against the
/// current [`HiveState`].
pub struct HiveController {
    desired: Option<HiveManifest>,
    current: HiveState,
}

impl HiveController {
    /// Create a controller with no desired state and an idle current state.
    pub fn new() -> Self {
        Self {
            desired: None,
            current: HiveState {
                running_agents: vec![],
                graph_status: "idle".into(),
                bus_status: "idle".into(),
            },
        }
    }

    /// Set the desired manifest for the hive.
    pub fn apply(&mut self, manifest: HiveManifest) -> Result<()> {
        self.desired = Some(manifest);
        Ok(())
    }

    /// Reconcile desired vs current state, returning a list of actions taken.
    pub fn reconcile(&mut self) -> Result<Vec<String>> {
        let Some(desired) = &self.desired else {
            return Ok(vec![]);
        };

        let mut actions = Vec::new();

        let current_map: std::collections::HashMap<&str, u32> = self
            .current
            .running_agents
            .iter()
            .map(|a| (a.name.as_str(), a.replicas))
            .collect();

        let desired_names: std::collections::HashSet<&str> =
            desired.agents.iter().map(|a| a.name.as_str()).collect();

        for agent in &desired.agents {
            match current_map.get(agent.name.as_str()) {
                Some(&replicas) if replicas != agent.replicas => {
                    actions.push(format!("scale {} to {}", agent.name, agent.replicas));
                }
                None => {
                    actions.push(format!("scale {} to {}", agent.name, agent.replicas));
                }
                _ => {}
            }
        }

        for current_agent in &self.current.running_agents {
            if !desired_names.contains(current_agent.name.as_str()) {
                actions.push(format!("remove {}", current_agent.name));
            }
        }

        self.current.running_agents = desired.agents.clone();
        Ok(actions)
    }

    /// Return a reference to the current hive state.
    pub fn status(&self) -> &HiveState {
        &self.current
    }

    /// Return the desired manifest, if one has been applied.
    pub fn desired_manifest(&self) -> Option<&HiveManifest> {
        self.desired.as_ref()
    }

    /// Scale a named agent to the given replica count.
    pub fn scale_agent(&mut self, name: &str, replicas: u32) -> Result<()> {
        let mut found = false;
        if let Some(manifest) = &mut self.desired
            && let Some(agent) = manifest.agents.iter_mut().find(|a| a.name == name)
        {
            agent.replicas = replicas;
            found = true;
        }
        if let Some(current) = self
            .current
            .running_agents
            .iter_mut()
            .find(|a| a.name == name)
        {
            current.replicas = replicas;
            found = true;
        }
        if found {
            Ok(())
        } else {
            Err(HiveError::Controller(format!("agent not found: {name}")))
        }
    }

    /// Remove a named agent from the hive. Returns whether it existed.
    pub fn remove_agent(&mut self, name: &str) -> Result<bool> {
        let mut found = false;
        if let Some(manifest) = &mut self.desired {
            let before = manifest.agents.len();
            manifest.agents.retain(|a| a.name != name);
            if manifest.agents.len() < before {
                found = true;
            }
        }
        let before = self.current.running_agents.len();
        self.current.running_agents.retain(|a| a.name != name);
        if self.current.running_agents.len() < before {
            found = true;
        }
        Ok(found)
    }
}

impl Default for HiveController {
    fn default() -> Self {
        Self::new()
    }
}
