//! Strategy router: entity-linked, multi-entity, infrastructure-relation,
//! aggregation retrieval, and source filtering.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

use super::constants::*;
use super::strategies::{
    concept_retrieval, entity_retrieval, extract_entity_ids, simple_retrieval, ENTITY_ID_RE,
};
use super::types::{Fact, MemorySearch};

static SUBNET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r#"(?i)subnet\s+named\s+['"]?([A-Za-z0-9_.\-]+)['"]?"#).expect("valid regex"),
        Regex::new(r"(?i)\b([A-Za-z0-9_.]+)\s+subnet\b").expect("valid regex"),
    ]
});

static MULTI_WORD_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b").expect("valid regex"));

// ── Entity-linked retrieval ─────────────────────────────────────────────

/// Retrieve all facts linked to structured entity IDs in the question.
pub fn entity_linked_retrieval(
    memory: &dyn MemorySearch, question: &str, existing_facts: &[Fact], local_only: bool,
) -> Vec<Fact> {
    let entity_ids = extract_entity_ids(question);
    if entity_ids.is_empty() {
        return existing_facts.to_vec();
    }

    let mut existing_ids: HashSet<String> = existing_facts
        .iter()
        .filter(|f| !f.experience_id.is_empty())
        .map(|f| f.experience_id.clone())
        .collect();
    let mut new_facts = Vec::new();

    let q_lower = question.to_lowercase();
    let is_incident = ["incident", "cve", "vulnerability", "security"]
        .iter()
        .any(|kw| q_lower.contains(kw));
    let search_limit = if is_incident {
        INCIDENT_QUERY_SEARCH_LIMIT
    } else {
        ENTITY_SEARCH_LIMIT
    };

    for entity_id in &entity_ids {
        // Text search
        let results = if local_only {
            memory.search_local(entity_id, search_limit)
        } else {
            memory.search(entity_id, search_limit)
        };
        collect_new_facts(&results, &mut existing_ids, &mut new_facts);

        // Entity retrieval path
        let results = if local_only {
            memory.retrieve_by_entity_local(entity_id, search_limit)
        } else {
            memory.retrieve_by_entity(entity_id, search_limit)
        };
        collect_new_facts(&results, &mut existing_ids, &mut new_facts);

        // For incident queries: concept search for exact ID
        if is_incident {
            let kw = vec![entity_id.clone()];
            let results = if local_only {
                memory.search_by_concept_local(&kw, CONCEPT_EXACT_SEARCH_LIMIT)
            } else {
                memory.search_by_concept(&kw, CONCEPT_EXACT_SEARCH_LIMIT)
            };
            collect_new_facts(&results, &mut existing_ids, &mut new_facts);
        }
    }

    if new_facts.is_empty() && local_only {
        return entity_linked_retrieval(memory, question, existing_facts, false);
    }

    let mut result = existing_facts.to_vec();
    result.extend(new_facts);
    result
}

// ── Multi-entity retrieval ──────────────────────────────────────────────

/// Chain-aware retrieval for questions mentioning 2+ named entities or IDs.
pub fn multi_entity_retrieval(
    memory: &dyn MemorySearch, question: &str, existing_facts: &[Fact], local_only: bool,
) -> Vec<Fact> {
    let name_candidates: Vec<String> = MULTI_WORD_NAME_RE
        .find_iter(question)
        .map(|m| m.as_str().to_string())
        .collect();
    let id_candidates = extract_entity_ids(question);

    let all_entities: HashSet<String> = name_candidates
        .into_iter()
        .chain(id_candidates)
        .collect();

    if all_entities.len() < 2 {
        return existing_facts.to_vec();
    }

    let mut existing_ids: HashSet<String> = existing_facts
        .iter()
        .filter(|f| !f.experience_id.is_empty())
        .map(|f| f.experience_id.clone())
        .collect();
    let mut new_facts = Vec::new();

    for entity in &all_entities {
        let results = if local_only {
            memory.retrieve_by_entity_local(entity, MULTI_ENTITY_LIMIT)
        } else {
            memory.retrieve_by_entity(entity, MULTI_ENTITY_LIMIT)
        };
        collect_new_facts(&results, &mut existing_ids, &mut new_facts);

        if ENTITY_ID_RE.is_match(entity) {
            let results = if local_only {
                memory.search_local(entity, ENTITY_ID_TEXT_SEARCH_LIMIT)
            } else {
                memory.search(entity, ENTITY_ID_TEXT_SEARCH_LIMIT)
            };
            collect_new_facts(&results, &mut existing_ids, &mut new_facts);
        }
    }

    let mut result = existing_facts.to_vec();
    result.extend(new_facts);
    result
}

// ── Infrastructure relation retrieval ───────────────────────────────────

/// Follow infrastructure relation targets (e.g. subnet → CIDR).
pub fn infrastructure_relation_retrieval(
    memory: &dyn MemorySearch, question: &str, existing_facts: &[Fact], local_only: bool,
) -> Vec<Fact> {
    let q_lower = question.to_lowercase();
    if !q_lower.contains("subnet") {
        return Vec::new();
    }

    let mut existing_ids: HashSet<String> = existing_facts.iter().map(|f| f.dedup_key()).collect();
    let mut seen_candidates: HashSet<String> = HashSet::new();
    let mut candidate_names = Vec::new();
    let skip_words: HashSet<&str> = ["subnet", "named", "in", "the", "on", "for"]
        .into_iter()
        .collect();

    for fact in existing_facts {
        let text = format!("{} {}", fact.context, fact.outcome);
        for pattern in SUBNET_PATTERNS.iter() {
            for cap in pattern.captures_iter(&text) {
                if let Some(m) = cap.get(1) {
                    let candidate = m.as_str().trim_matches(|c: char| {
                        c == ' ' || c == '\'' || c == '"' || c == '.' || c == ','
                            || c == ':' || c == ';' || c == '(' || c == ')'
                    });
                    if candidate.is_empty() || skip_words.contains(candidate.to_lowercase().as_str())
                    {
                        continue;
                    }
                    if seen_candidates.insert(candidate.to_lowercase()) {
                        candidate_names.push(candidate.to_string());
                    }
                }
            }
        }
    }

    let mut new_facts = Vec::new();
    for candidate in &candidate_names {
        let mut results = if local_only {
            memory.retrieve_by_entity_local(candidate, ENTITY_SEARCH_LIMIT)
        } else {
            memory.retrieve_by_entity(candidate, ENTITY_SEARCH_LIMIT)
        };
        if results.is_empty() && local_only {
            results = memory.retrieve_by_entity(candidate, ENTITY_SEARCH_LIMIT);
        }
        for fact in results {
            let key = fact.dedup_key();
            if existing_ids.insert(key) {
                new_facts.push(fact);
            }
        }
    }
    new_facts
}

// ── Supplement simple retrieval ─────────────────────────────────────────

/// Add focused entity/concept hits back into large simple retrievals.
pub fn supplement_simple_retrieval(
    memory: &dyn MemorySearch, question: &str, existing_facts: &[Fact], local_only: bool,
) -> Vec<Fact> {
    let mut existing_keys: HashSet<String> = existing_facts.iter().map(|f| f.dedup_key()).collect();
    let mut supplemented: Vec<Fact> = existing_facts.to_vec();

    let mut targeted: Vec<Fact> = entity_retrieval(memory, question, local_only);
    targeted.extend(concept_retrieval(memory, question, local_only));
    targeted.extend(infrastructure_relation_retrieval(
        memory,
        question,
        &supplemented,
        local_only,
    ));

    if targeted.is_empty() && local_only {
        targeted = entity_retrieval(memory, question, false);
        targeted.extend(concept_retrieval(memory, question, false));
    }

    let mut added = 0;
    for fact in targeted {
        if existing_keys.insert(fact.dedup_key()) {
            supplemented.push(fact);
            added += 1;
        }
    }

    if added > 0 {
        tracing::info!(
            added,
            question = &question[..question.len().min(60)],
            "simple retrieval supplements"
        );
    }
    supplemented
}

// ── Aggregation retrieval ───────────────────────────────────────────────

/// Handle meta-memory questions via graph aggregation.
pub fn aggregation_retrieval(memory: &dyn MemorySearch, question: &str) -> Vec<Fact> {
    if !memory.supports_hierarchical() {
        let (facts, _) = simple_retrieval(memory, question, false, None);
        return facts;
    }

    let q_lower = question.to_lowercase();
    let mut results = Vec::new();
    let entity_kw = ["project", "people", "person", "team", "member"]
        .iter()
        .find(|kw| q_lower.contains(**kw))
        .copied()
        .unwrap_or("");

    if entity_kw == "project" {
        let agg = memory.execute_aggregation("list_concepts", Some("project"));
        if !agg.items.is_empty() {
            results.push(agg_fact("Meta-memory: Project count", &format!(
                "There are {} distinct project-related concepts: {}",
                agg.items.len(), agg.items.join(", ")
            )));
        }
    }
    if ["people", "person", "member", "team"].contains(&entity_kw) {
        let agg = memory.execute_aggregation("list_entities", None);
        if !agg.items.is_empty() {
            results.push(agg_fact("Meta-memory: Entity list", &format!(
                "There are {} distinct entities: {}",
                agg.items.len(), agg.items.join(", ")
            )));
        }
    }

    // Conflicting topics
    if ["conflict", "contradict", "disagree", "different sources"]
        .iter()
        .any(|kw| q_lower.contains(kw))
    {
        let agg = memory.execute_aggregation("list_superseded", None);
        if !agg.items.is_empty() {
            let topics: Vec<_> = agg.items.iter().take(CONFLICTING_TOPICS_LIMIT).collect();
            let joined = topics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
            results.push(agg_fact("Meta-memory: Conflicting topics",
                &format!("Topics with conflicting/updated information: {joined}")));
        }
    }

    // General fallback
    if results.is_empty() {
        let total_agg = memory.execute_aggregation("count_total", None);
        let entity_agg = memory.execute_aggregation("list_entities", None);
        let concept_agg = memory.execute_aggregation("count_by_concept", None);
        let mut parts = Vec::new();
        if let Some(c) = total_agg.count {
            parts.push(format!("Total facts stored: {c}"));
        }
        if !entity_agg.items.is_empty() {
            parts.push(format!(
                "Distinct entities ({}): {}",
                entity_agg.items.len(),
                entity_agg.items.iter().take(30).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !concept_agg.item_counts.is_empty() {
            let top: Vec<_> = concept_agg
                .item_counts
                .iter()
                .take(20)
                .map(|(c, n)| format!("{c} ({n} facts)"))
                .collect();
            parts.push(format!("Top concepts: {}", top.join(", ")));
        }
        if !parts.is_empty() {
            results.push(agg_fact("Meta-memory: Knowledge summary", &parts.join(". ")));
        }
    }

    // Include regular facts for context
    let (regular, _) = simple_retrieval(memory, question, false, None);
    results.extend(regular);
    results
}

/// Filter facts to those from a specific source referenced in the question.
pub fn filter_facts_by_source_reference(question: &str, facts: &[Fact]) -> Vec<Fact> {
    let q_lower = question.to_lowercase();
    let patterns = [
        "mentioned in the ",
        "from the ",
        "in the ",
        "according to the ",
    ];
    let end_words = ["article", "report", "source", "piece", "?"];

    let mut source_keywords = Vec::new();
    for pattern in &patterns {
        if let Some(idx) = q_lower.find(pattern) {
            let after = &q_lower[idx + pattern.len()..];
            for end_word in &end_words {
                if let Some(end_idx) = after.find(end_word)
                    && end_idx > 0
                {
                    source_keywords.push(after[..end_idx].trim().to_string());
                    break;
                }
            }
        }
    }

    if source_keywords.is_empty() {
        return Vec::new();
    }

    facts
        .iter()
        .filter(|f| {
            let source = f
                .metadata
                .get("source_label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            source_keywords
                .iter()
                .any(|kw| !kw.is_empty() && source.contains(kw))
        })
        .cloned()
        .collect()
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a meta-memory aggregation fact.
fn agg_fact(context: &str, outcome: &str) -> Fact {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("aggregation".into(), serde_json::Value::Bool(true));
    Fact {
        context: context.into(), outcome: outcome.into(), confidence: 1.0,
        timestamp: String::new(), experience_id: String::new(),
        tags: vec!["meta_memory".into()], metadata,
    }
}

/// Collect new facts from results into `new_facts`, skipping already-seen IDs.
fn collect_new_facts(results: &[Fact], seen: &mut HashSet<String>, new_facts: &mut Vec<Fact>) {
    for fact in results {
        if !fact.experience_id.is_empty() && seen.insert(fact.experience_id.clone()) {
            new_facts.push(fact.clone());
        }
    }
}
