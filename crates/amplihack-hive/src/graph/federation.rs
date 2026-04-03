use std::collections::HashMap;

use chrono::Utc;

use super::{HiveGraph, BROADCAST_TAG_PREFIX, ESCALATION_TAG_PREFIX};

impl HiveGraph {
    /// Set the parent hive ID.
    pub fn set_parent(&mut self, parent_id: impl Into<String>) {
        self.parent_id = Some(parent_id.into());
    }

    /// Return the parent hive ID, if any.
    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// Add a child hive ID.
    pub fn add_child(&mut self, child_id: impl Into<String>) {
        self.children_ids.push(child_id.into());
    }

    /// Return the child hive IDs.
    pub fn children_ids(&self) -> &[String] {
        &self.children_ids
    }

    /// Escalate a fact to a parent hive by copying it with an escalation tag.
    /// Returns `None` if the fact is not found or already has an
    /// escalation/broadcast tag.
    pub fn escalate_fact(
        &self,
        fact_id: &str,
        parent: &mut HiveGraph,
    ) -> Option<String> {
        let fact = self.facts.iter().find(|f| f.fact_id == fact_id)?;
        if fact
            .tags
            .iter()
            .any(|t| t.starts_with(ESCALATION_TAG_PREFIX) || t.starts_with(BROADCAST_TAG_PREFIX))
        {
            return None;
        }
        let mut new_tags = fact.tags.clone();
        new_tags.push(format!("{}{}", ESCALATION_TAG_PREFIX, self.hive_id));
        let mut new_fact = fact.clone();
        new_fact.fact_id = uuid::Uuid::new_v4().to_string();
        new_fact.tags = new_tags;
        new_fact.created_at = Utc::now();
        let new_id = new_fact.fact_id.clone();
        parent.facts.push(new_fact);
        Some(new_id)
    }

    /// Broadcast a fact to all child hives.
    /// Skips if the fact already has a broadcast or escalation tag.
    /// Returns a map of child_hive_id -> new_fact_id.
    pub fn broadcast_fact(
        &self,
        fact_id: &str,
        children: &mut [HiveGraph],
    ) -> HashMap<String, String> {
        let mut result = HashMap::new();
        let fact = match self.facts.iter().find(|f| f.fact_id == fact_id) {
            Some(f) => f,
            None => return result,
        };
        if fact
            .tags
            .iter()
            .any(|t| t.starts_with(BROADCAST_TAG_PREFIX) || t.starts_with(ESCALATION_TAG_PREFIX))
        {
            return result;
        }
        for child in children.iter_mut() {
            let mut new_tags = fact.tags.clone();
            new_tags.push(format!("{}{}", BROADCAST_TAG_PREFIX, self.hive_id));
            let mut new_fact = fact.clone();
            new_fact.fact_id = uuid::Uuid::new_v4().to_string();
            new_fact.tags = new_tags;
            new_fact.created_at = Utc::now();
            let new_id = new_fact.fact_id.clone();
            child.facts.push(new_fact);
            result.insert(child.hive_id.clone(), new_id);
        }
        result
    }
}
