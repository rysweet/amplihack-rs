//! Federation support for multi-hive topologies.

use std::collections::HashMap;

use super::{BROADCAST_TAG_PREFIX, ESCALATION_TAG_PREFIX, HiveGraph};

impl HiveGraph {
    pub fn set_parent(&mut self, parent_id: &str) {
        self.parent_id = Some(parent_id.to_string());
    }

    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    pub fn add_child(&mut self, child_id: &str) {
        if !self.children_ids.contains(&child_id.to_string()) {
            self.children_ids.push(child_id.to_string());
        }
    }

    pub fn children_ids(&self) -> &[String] {
        &self.children_ids
    }

    pub fn escalate_fact(&self, fact_id: &str, parent: &mut HiveGraph) -> Option<String> {
        let fact = self.facts.iter().find(|f| f.fact_id == fact_id)?;
        if fact
            .tags
            .iter()
            .any(|t| t.starts_with(ESCALATION_TAG_PREFIX) || t.starts_with(BROADCAST_TAG_PREFIX))
        {
            return None;
        }
        let mut tags = fact.tags.clone();
        tags.push(format!("{ESCALATION_TAG_PREFIX}{}", self.hive_id));
        parent
            .store_fact(
                &fact.concept,
                &fact.content,
                fact.confidence,
                &fact.source_id,
                tags,
            )
            .ok()
    }

    pub fn broadcast_fact(
        &self,
        fact_id: &str,
        children: &mut [HiveGraph],
    ) -> HashMap<String, String> {
        let mut result = HashMap::new();
        let fact = match self.facts.iter().find(|f| f.fact_id == fact_id) {
            Some(f) => f.clone(),
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
            let mut tags = fact.tags.clone();
            tags.push(format!("{BROADCAST_TAG_PREFIX}{}", self.hive_id));
            if let Ok(new_id) = child.store_fact(
                &fact.concept,
                &fact.content,
                fact.confidence,
                &fact.source_id,
                tags,
            ) {
                result.insert(child.hive_id.clone(), new_id);
            }
        }
        result
    }
}
