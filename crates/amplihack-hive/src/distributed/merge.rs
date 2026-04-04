//! Merge utilities — RRF merge, aggregation merge, and relevance scoring.

use super::{BIGRAM_WEIGHT, UNIGRAM_WEIGHT};
use crate::dht::ShardFact;
use std::collections::{HashMap, HashSet};

/// Merge ranked results from multiple shards using position-based scoring.
pub fn merge_ranked_shard_results(
    results_by_agent: &[(String, Vec<ShardFact>)],
    limit: usize,
    pos_decrement: f64,
) -> Vec<ShardFact> {
    let mut best: HashMap<String, (f64, ShardFact)> = HashMap::new();
    let mut sorted: Vec<&(String, Vec<ShardFact>)> = results_by_agent.iter().collect();
    sorted.sort_by_key(|(aid, _)| aid.clone());

    for (_, facts) in sorted {
        for (rank, fact) in facts.iter().enumerate() {
            let score = (1.0 - rank as f64 * pos_decrement).max(0.0);
            let key = content_hash(&fact.content);
            best.entry(key)
                .and_modify(|(s, f)| {
                    if score > *s || (score == *s && tiebreak(fact) < tiebreak(f)) {
                        *s = score;
                        *f = fact.clone();
                    }
                })
                .or_insert_with(|| (score, fact.clone()));
        }
    }

    let mut merged: Vec<(f64, ShardFact)> = best.into_values().collect();
    merged.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| tiebreak(&a.1).cmp(&tiebreak(&b.1)))
    });
    merged.into_iter().take(limit).map(|(_, f)| f).collect()
}

fn content_hash(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut h);
    format!("{:x}", h.finish())
}

fn tiebreak(f: &ShardFact) -> (String, String, String) {
    (f.source_agent.clone(), f.fact_id.clone(), f.content.clone())
}

/// Merge aggregation results from multiple shards.
pub fn merge_aggregation_results(
    query_type: &str,
    results: &[HashMap<String, serde_json::Value>],
) -> HashMap<String, serde_json::Value> {
    let mut merged = HashMap::new();
    match query_type {
        "count_total" => {
            let total: u64 = results
                .iter()
                .filter_map(|r| r.get("count").and_then(|v| v.as_u64()))
                .sum();
            merged.insert("count".into(), serde_json::json!(total));
        }
        "list_entities" | "list_concepts" | "list_superseded" => {
            let mut items: Vec<String> = results
                .iter()
                .filter_map(|r| r.get(query_type).and_then(|v| v.as_array()))
                .flatten()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            items.sort();
            merged.insert(query_type.into(), serde_json::json!(items));
        }
        "count_by_concept" => {
            let mut counts: HashMap<String, u64> = HashMap::new();
            for r in results {
                if let Some(obj) = r.get("counts").and_then(|v| v.as_object()) {
                    for (k, v) in obj {
                        *counts.entry(k.clone()).or_default() += v.as_u64().unwrap_or(0);
                    }
                }
            }
            merged.insert("counts".into(), serde_json::json!(counts));
        }
        _ => {
            if let Some(first) = results.iter().find(|r| !r.is_empty()) {
                return first.clone();
            }
        }
    }
    merged
}

/// Compute relevance of `content` to `query` via unigram + bigram overlap.
pub fn relevance_score(content: &str, query: &str) -> f64 {
    if query.is_empty() || content.is_empty() {
        return 0.0;
    }
    let qt: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let cl = content.to_lowercase();
    let cw: HashSet<String> = cl.split_whitespace().map(|w| w.to_string()).collect();

    let uhits = qt.iter().filter(|t| cw.contains(t.as_str())).count();
    let uscore = uhits as f64 / qt.len().max(1) as f64;

    let qbigrams: Vec<(String, String)> = qt
        .windows(2)
        .map(|w| (w[0].clone(), w[1].clone()))
        .collect();
    let cwl: Vec<String> = cl.split_whitespace().map(|w| w.to_string()).collect();
    let cbigrams: HashSet<(&str, &str)> = cwl
        .windows(2)
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();
    let bhits = qbigrams
        .iter()
        .filter(|(a, b)| cbigrams.contains(&(a.as_str(), b.as_str())))
        .count();
    let bscore = bhits as f64 / qbigrams.len().max(1) as f64;

    uscore * UNIGRAM_WEIGHT + bscore * BIGRAM_WEIGHT
}

/// Extract textual content from a result map.
pub fn extract_content(result: &serde_json::Value) -> String {
    for key in &["outcome", "content", "fact"] {
        if let Some(s) = result.get(key).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

/// Merge local + hive fact lists with dedup and relevance scoring.
pub fn merge_fact_lists(
    local: &[serde_json::Value],
    hive: &[serde_json::Value],
    limit: usize,
    query: &str,
) -> Vec<serde_json::Value> {
    let mut seen = HashSet::new();
    let mut all: Vec<(f64, serde_json::Value)> = Vec::new();
    for item in local.iter().chain(hive.iter()) {
        let c = extract_content(item);
        let h = content_hash(&c);
        if seen.insert(h) {
            let score = if query.is_empty() {
                0.5
            } else {
                relevance_score(&c, query)
            };
            all.push((score, item.clone()));
        }
    }
    all.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| extract_content(&a.1).cmp(&extract_content(&b.1)))
    });
    all.into_iter().take(limit).map(|(_, v)| v).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sfact(id: &str, content: &str, agent: &str) -> ShardFact {
        ShardFact {
            fact_id: id.into(),
            content: content.into(),
            source_agent: agent.into(),
            ..ShardFact::new(id, content)
        }
    }

    #[test]
    fn rrf_merge_deduplicates() {
        let r = vec![
            ("a1".into(), vec![sfact("f1", "same content", "a1")]),
            ("a2".into(), vec![sfact("f2", "same content", "a2")]),
        ];
        assert_eq!(merge_ranked_shard_results(&r, 10, 0.05).len(), 1);
    }

    #[test]
    fn rrf_merge_respects_limit() {
        let r = vec![(
            "a1".into(),
            vec![
                sfact("f1", "r1", "a1"),
                sfact("f2", "r2", "a1"),
                sfact("f3", "r3", "a1"),
            ],
        )];
        assert_eq!(merge_ranked_shard_results(&r, 2, 0.1).len(), 2);
    }

    #[test]
    fn aggregation_count_total() {
        let results = vec![
            [("count".into(), serde_json::json!(5))].into(),
            [("count".into(), serde_json::json!(3))].into(),
        ];
        assert_eq!(
            merge_aggregation_results("count_total", &results)["count"],
            8
        );
    }

    #[test]
    fn aggregation_list_entities() {
        let results = vec![
            [("list_entities".into(), serde_json::json!(["a", "b"]))].into(),
            [("list_entities".into(), serde_json::json!(["b", "c"]))].into(),
        ];
        let m = merge_aggregation_results("list_entities", &results);
        assert_eq!(m["list_entities"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn relevance_score_full_match() {
        assert!(relevance_score("Rust memory safety", "Rust memory safety") > 0.9);
    }

    #[test]
    fn relevance_score_no_match() {
        assert!(relevance_score("Python is great", "Rust memory") < 0.01);
    }

    #[test]
    fn merge_fact_lists_dedup() {
        let local = vec![serde_json::json!({"content": "fact A"})];
        let hive = vec![
            serde_json::json!({"content": "fact A"}),
            serde_json::json!({"content": "fact B"}),
        ];
        assert_eq!(merge_fact_lists(&local, &hive, 10, "fact").len(), 2);
    }

    #[test]
    fn extract_content_tries_keys() {
        assert_eq!(extract_content(&serde_json::json!({"outcome": "x"})), "x");
        assert_eq!(extract_content(&serde_json::json!({"content": "y"})), "y");
        assert_eq!(extract_content(&serde_json::json!({"other": "w"})), "");
    }
}
