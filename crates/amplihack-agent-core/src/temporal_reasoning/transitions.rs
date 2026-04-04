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
