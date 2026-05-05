//! Transition chain building and collapsing logic.
//!
//! Split from the parent `temporal_reasoning` module to stay under 400 LOC.

use std::collections::{HashMap, HashSet};

use amplihack_memory::Fact;

use super::extract_temporal_state_values;

// ---------------------------------------------------------------------------
// Transition-chain entry
// ---------------------------------------------------------------------------

/// A single entry in a temporal transition chain.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Transition {
    pub value: String,
    pub timestamp: String,
    pub temporal_index: i64,
    pub experience_id: String,
    pub sequence_position: usize,
    pub superseded: bool,
    pub metadata: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Build chain from facts
// ---------------------------------------------------------------------------

/// Build a transition chain from candidate facts for a given entity/field.
pub fn transition_chain_from_facts(entity: &str, field: &str, facts: &[Fact]) -> Vec<Transition> {
    let entity_lower = entity.to_lowercase();
    let field_lower = field.to_lowercase();
    let mut chain = Vec::new();
    let mut seen: HashSet<(String, String, i64, String, bool)> = HashSet::new();

    for (fact_index, fact) in facts.iter().enumerate() {
        let context = fact.context.to_lowercase();
        let outcome = &fact.outcome;
        let combined = format!("{context} {}", outcome.to_lowercase());
        if !combined.contains(&entity_lower) || !combined.contains(&field_lower) {
            continue;
        }

        let temporal_index = fact
            .metadata
            .get("temporal_index")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let exp_id = if !fact.experience_id.is_empty() {
            fact.experience_id.clone()
        } else {
            format!("fact-{fact_index}")
        };
        let extracted = extract_temporal_state_values(outcome, field);

        if extracted.len() > 1 {
            append_multi_value_entries(
                &mut chain,
                &mut seen,
                &extracted,
                fact,
                temporal_index,
                &exp_id,
            );
            continue;
        }

        if extracted.is_empty() && (field_lower == "date" || field_lower == "deadline") {
            continue;
        }

        let atomic = if extracted.is_empty() {
            outcome.to_string()
        } else {
            extracted[0].clone()
        };
        let sup = fact
            .metadata
            .get("superseded")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let key = (
            exp_id.clone(),
            atomic.to_lowercase(),
            temporal_index,
            fact.timestamp.clone(),
            sup,
        );
        if !seen.insert(key) {
            continue;
        }
        chain.push(Transition {
            value: atomic,
            timestamp: fact.timestamp.clone(),
            temporal_index,
            experience_id: exp_id,
            sequence_position: 0,
            superseded: sup,
            metadata: fact.metadata.clone(),
        });
    }

    chain.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then(a.temporal_index.cmp(&b.temporal_index))
            .then(a.sequence_position.cmp(&b.sequence_position))
    });
    chain
}

fn append_multi_value_entries(
    chain: &mut Vec<Transition>,
    seen: &mut HashSet<(String, String, i64, String, bool)>,
    extracted: &[String],
    fact: &Fact,
    temporal_index: i64,
    exp_id: &str,
) {
    for (offset, sv) in extracted.iter().enumerate() {
        let sup = offset < extracted.len() - 1
            || fact
                .metadata
                .get("superseded")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        let key = (
            exp_id.to_string(),
            sv.to_lowercase(),
            temporal_index,
            fact.timestamp.clone(),
            sup,
        );
        if !seen.insert(key) {
            continue;
        }
        chain.push(Transition {
            value: sv.clone(),
            timestamp: fact.timestamp.clone(),
            temporal_index,
            experience_id: exp_id.to_string(),
            sequence_position: offset,
            superseded: sup,
            metadata: fact.metadata.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Collapse helpers
// ---------------------------------------------------------------------------

/// Collapse change-count transitions by deduplicating values.
pub fn collapse_change_count_transitions(
    transitions: &[Transition],
    field: &str,
) -> Vec<Transition> {
    let mut seen = HashSet::new();
    let mut collapsed = Vec::new();
    for t in transitions {
        let vals = extract_temporal_state_values(&t.value, field);
        if vals.is_empty() {
            continue;
        }
        for sv in vals {
            let key = sv.to_lowercase();
            if seen.insert(key) {
                let mut entry = t.clone();
                entry.value = sv;
                collapsed.push(entry);
            }
        }
    }
    if collapsed.is_empty() {
        transitions.to_vec()
    } else {
        collapsed
    }
}

/// Topologically collapse temporal-lookup transitions.
pub fn collapse_temporal_lookup_transitions(transitions: &[Transition]) -> Vec<Transition> {
    let mut reps: HashMap<String, &Transition> = HashMap::new();
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    let mut indeg: HashMap<String, usize> = HashMap::new();
    let mut grouped: HashMap<String, Vec<(usize, usize, String)>> = HashMap::new();

    for (idx, t) in transitions.iter().enumerate() {
        let val = t.value.trim();
        if val.is_empty() {
            continue;
        }
        let key = val.to_lowercase();
        reps.entry(key.clone()).or_insert(t);
        first_seen.entry(key.clone()).or_insert(idx);
        indeg.entry(key.clone()).or_insert(0);
        let eid = if !t.experience_id.is_empty() {
            t.experience_id.clone()
        } else {
            format!("transition-{idx}")
        };
        grouped
            .entry(eid)
            .or_default()
            .push((t.sequence_position, idx, key));
    }

    build_adjacency(&grouped, &mut adj, &mut indeg);
    let ordered_keys = topo_sort(&first_seen, &adj, &mut indeg, &reps);

    let out: Vec<Transition> = ordered_keys
        .iter()
        .filter_map(|k| reps.get(k).map(|t| (*t).clone()))
        .collect();
    if out.is_empty() {
        transitions.to_vec()
    } else {
        out
    }
}

fn build_adjacency(
    grouped: &HashMap<String, Vec<(usize, usize, String)>>,
    adj: &mut HashMap<String, HashSet<String>>,
    indeg: &mut HashMap<String, usize>,
) {
    for seq in grouped.values() {
        let mut sorted = seq.clone();
        sorted.sort();
        let mut ordered: Vec<String> = Vec::new();
        for (_, _, k) in &sorted {
            if ordered.last() != Some(k) {
                ordered.push(k.clone());
            }
        }
        for pair in ordered.windows(2) {
            let (prev, next) = (&pair[0], &pair[1]);
            if prev == next {
                continue;
            }
            if adj.entry(prev.clone()).or_default().insert(next.clone()) {
                *indeg.entry(next.clone()).or_insert(0) += 1;
            }
        }
    }
}

fn topo_sort(
    first_seen: &HashMap<String, usize>,
    adj: &HashMap<String, HashSet<String>>,
    indeg: &mut HashMap<String, usize>,
    reps: &HashMap<String, &Transition>,
) -> Vec<String> {
    let mut ready: Vec<String> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    ready.sort_by_key(|k| first_seen.get(k).copied().unwrap_or(usize::MAX));

    let mut ordered_keys = Vec::new();
    while let Some(key) = ready.first().cloned() {
        ready.remove(0);
        ordered_keys.push(key.clone());
        if let Some(neighbors) = adj.get(&key) {
            let mut sorted_n: Vec<_> = neighbors.iter().cloned().collect();
            sorted_n.sort_by_key(|k| first_seen.get(k).copied().unwrap_or(usize::MAX));
            for nk in sorted_n {
                if let Some(d) = indeg.get_mut(&nk) {
                    *d -= 1;
                    if *d == 0 {
                        let fs = first_seen.get(&nk).copied().unwrap_or(usize::MAX);
                        let pos = ready
                            .iter()
                            .position(|r| first_seen.get(r).copied().unwrap_or(usize::MAX) > fs);
                        match pos {
                            Some(p) => ready.insert(p, nk),
                            None => ready.push(nk),
                        }
                    }
                }
            }
        }
    }

    if ordered_keys.len() != indeg.len() {
        ordered_keys = reps.keys().cloned().collect();
        ordered_keys.sort_by_key(|k| first_seen.get(k).copied().unwrap_or(usize::MAX));
    }

    ordered_keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use amplihack_memory::Fact;
    use serde_json::json;

    fn make_fact(
        context: &str,
        outcome: &str,
        timestamp: &str,
        experience_id: &str,
        temporal_index: i64,
    ) -> Fact {
        let mut f = Fact::new(context, outcome);
        f.timestamp = timestamp.to_string();
        f.experience_id = experience_id.to_string();
        f.metadata
            .insert("temporal_index".to_string(), json!(temporal_index));
        f
    }

    // ---- transition_chain_from_facts ----

    #[test]
    fn empty_facts_produce_empty_chain() {
        let chain = transition_chain_from_facts("user", "status", &[]);
        assert!(chain.is_empty());
    }

    #[test]
    fn no_matching_facts_produce_empty_chain() {
        let facts = vec![make_fact(
            "project alpha",
            "completed",
            "2025-01-01",
            "e1",
            0,
        )];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert!(chain.is_empty());
    }

    #[test]
    fn single_matching_fact_produces_one_entry() {
        let facts = vec![make_fact(
            "user status update",
            "active",
            "2025-01-01",
            "e1",
            0,
        )];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].value, "active");
        assert_eq!(chain[0].experience_id, "e1");
    }

    #[test]
    fn chain_sorted_by_timestamp() {
        let facts = vec![
            make_fact("user status", "offline", "2025-01-03", "e2", 0),
            make_fact("user status", "online", "2025-01-01", "e1", 0),
            make_fact("user status", "away", "2025-01-02", "e3", 0),
        ];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].value, "online");
        assert_eq!(chain[1].value, "away");
        assert_eq!(chain[2].value, "offline");
    }

    #[test]
    fn chain_sorted_by_temporal_index_within_same_timestamp() {
        let facts = vec![
            make_fact("user status", "second", "2025-01-01", "e2", 2),
            make_fact("user status", "first", "2025-01-01", "e1", 1),
        ];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].value, "first");
        assert_eq!(chain[1].value, "second");
    }

    #[test]
    fn deduplicates_identical_entries() {
        let facts = vec![
            make_fact("user status", "active", "2025-01-01", "e1", 0),
            make_fact("user status", "active", "2025-01-01", "e1", 0),
        ];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn superseded_flag_read_from_metadata() {
        let mut fact = make_fact("user status", "old-value", "2025-01-01", "e1", 0);
        fact.metadata.insert("superseded".to_string(), json!(true));
        let chain = transition_chain_from_facts("user", "status", &[fact]);
        assert_eq!(chain.len(), 1);
        assert!(chain[0].superseded);
    }

    #[test]
    fn empty_experience_id_generates_synthetic() {
        let facts = vec![make_fact("user status", "active", "2025-01-01", "", 0)];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert_eq!(chain.len(), 1);
        assert!(chain[0].experience_id.starts_with("fact-"));
    }

    #[test]
    fn case_insensitive_entity_field_matching() {
        let facts = vec![make_fact(
            "USER Status Change",
            "active",
            "2025-01-01",
            "e1",
            0,
        )];
        let chain = transition_chain_from_facts("user", "status", &facts);
        assert_eq!(chain.len(), 1);
    }

    // ---- collapse_change_count_transitions ----

    #[test]
    fn collapse_empty_returns_empty() {
        let result = collapse_change_count_transitions(&[], "status");
        assert!(result.is_empty());
    }

    #[test]
    fn collapse_preserves_unique_values() {
        let transitions = vec![
            Transition {
                value: "alpha".to_string(),
                timestamp: "2025-01-01".to_string(),
                temporal_index: 0,
                experience_id: "e1".to_string(),
                sequence_position: 0,
                superseded: false,
                metadata: HashMap::new(),
            },
            Transition {
                value: "beta".to_string(),
                timestamp: "2025-01-02".to_string(),
                temporal_index: 0,
                experience_id: "e2".to_string(),
                sequence_position: 0,
                superseded: false,
                metadata: HashMap::new(),
            },
        ];
        // extract_temporal_state_values won't find sub-values for "status" in
        // plain words, so collapse returns the original vec.
        let result = collapse_change_count_transitions(&transitions, "status");
        assert_eq!(result.len(), 2);
    }

    // ---- collapse_temporal_lookup_transitions ----

    #[test]
    fn temporal_lookup_empty_returns_empty() {
        let result = collapse_temporal_lookup_transitions(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn temporal_lookup_deduplicates_same_value() {
        let t = Transition {
            value: "active".to_string(),
            timestamp: "2025-01-01".to_string(),
            temporal_index: 0,
            experience_id: "e1".to_string(),
            sequence_position: 0,
            superseded: false,
            metadata: HashMap::new(),
        };
        let transitions = vec![t.clone(), t];
        let result = collapse_temporal_lookup_transitions(&transitions);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, "active");
    }

    #[test]
    fn temporal_lookup_preserves_order_of_distinct_values() {
        let make = |val: &str, seq: usize, eid: &str| Transition {
            value: val.to_string(),
            timestamp: "2025-01-01".to_string(),
            temporal_index: 0,
            experience_id: eid.to_string(),
            sequence_position: seq,
            superseded: false,
            metadata: HashMap::new(),
        };
        let transitions = vec![
            make("first", 0, "e1"),
            make("second", 1, "e1"),
            make("third", 2, "e1"),
        ];
        let result = collapse_temporal_lookup_transitions(&transitions);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].value, "first");
        assert_eq!(result[1].value, "second");
        assert_eq!(result[2].value, "third");
    }

    #[test]
    fn temporal_lookup_skips_empty_values() {
        let t = Transition {
            value: "  ".to_string(),
            timestamp: "2025-01-01".to_string(),
            temporal_index: 0,
            experience_id: "e1".to_string(),
            sequence_position: 0,
            superseded: false,
            metadata: HashMap::new(),
        };
        let result = collapse_temporal_lookup_transitions(&[t]);
        // Whitespace-only values are skipped, so fallback to original
        assert_eq!(result.len(), 1);
    }
}
