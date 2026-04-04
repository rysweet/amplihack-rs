//! Adaptive retrieval strategies based on detected intent.

use std::collections::HashSet;
use tracing::info;

use super::types::{DetectedIntent, SynthesisConfig};
use crate::agentic_loop::traits::MemoryRetriever;
use crate::agentic_loop::types::MemoryFact;

/// Result of adaptive retrieval.
#[derive(Debug, Clone)]
pub struct RetrievalOutcome {
    pub facts: Vec<MemoryFact>,
    pub used_simple_path: bool,
    pub exhaustive: bool,
}

/// Run adaptive retrieval for `question` using the classified `intent`.
pub fn adaptive_retrieve(
    question: &str,
    intent: &DetectedIntent,
    retriever: &dyn MemoryRetriever,
    config: &SynthesisConfig,
) -> RetrievalOutcome {
    if intent.intent.is_aggregation() {
        let facts = retriever.search(question, config.max_retrieval_limit);
        return RetrievalOutcome {
            exhaustive: false,
            used_simple_path: false,
            facts,
        };
    }

    let use_simple = intent.intent.is_simple() || is_small_kb(retriever, question, config);
    if use_simple {
        let facts = retriever.search(question, config.max_retrieval_limit);
        let exhaustive = facts.len() < config.simple_retrieval_threshold;
        return RetrievalOutcome {
            used_simple_path: true,
            exhaustive,
            facts,
        };
    }

    let mut facts = retriever.search(question, config.max_retrieval_limit.min(200));
    facts.retain(|f| !is_qa_echo(f));

    if facts.is_empty() {
        info!(
            question = &question[..question.len().min(50)],
            "Entity retrieval empty; falling back to simple retrieval"
        );
        let facts = retriever.search(question, config.max_retrieval_limit);
        return RetrievalOutcome {
            used_simple_path: true,
            exhaustive: false,
            facts,
        };
    }
    RetrievalOutcome {
        used_simple_path: false,
        exhaustive: false,
        facts,
    }
}

/// Supplement with keyword-expanded retrieval for sparse fact sets.
pub fn supplement_keyword_expanded(
    question: &str,
    existing: &mut Vec<MemoryFact>,
    retriever: &dyn MemoryRetriever,
    config: &SynthesisConfig,
) {
    if existing.len() >= config.keyword_expansion_sparse_threshold {
        return;
    }
    let seen: HashSet<String> = existing.iter().map(|f| f.id.clone()).collect();
    for kw in extract_supplement_keywords(question) {
        for f in retriever.search(&kw, 50) {
            if !f.id.is_empty() && !seen.contains(&f.id) {
                existing.push(f);
            }
        }
    }
}

/// Multi-entity retrieval: when 2+ entities mentioned, retrieve per entity and merge.
pub fn multi_entity_supplement(
    question: &str,
    existing: &mut Vec<MemoryFact>,
    retriever: &dyn MemoryRetriever,
) {
    let entities = extract_entity_mentions(question);
    if entities.len() < 2 {
        return;
    }
    let seen: HashSet<String> = existing.iter().map(|f| f.id.clone()).collect();
    for entity in &entities {
        for f in retriever.search(entity, 50) {
            if !f.id.is_empty() && !seen.contains(&f.id) {
                existing.push(f);
            }
        }
    }
}

/// Remove duplicate facts by ID and filter out Q&A echo facts.
pub fn dedup_and_filter(facts: Vec<MemoryFact>) -> Vec<MemoryFact> {
    let mut seen = HashSet::new();
    facts
        .into_iter()
        .filter(|f| {
            if is_qa_echo(f) {
                return false;
            }
            if f.id.is_empty() {
                return true;
            }
            seen.insert(f.id.clone())
        })
        .collect()
}

fn is_small_kb(retriever: &dyn MemoryRetriever, question: &str, config: &SynthesisConfig) -> bool {
    retriever
        .search(question, config.simple_retrieval_threshold + 1)
        .len()
        <= config.simple_retrieval_threshold
}

fn is_qa_echo(fact: &MemoryFact) -> bool {
    fact.context.starts_with("Question:")
        && fact
            .metadata
            .get("tags")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| arr.iter().any(|t| t.as_str() == Some("q_and_a")))
}

fn extract_supplement_keywords(question: &str) -> Vec<String> {
    let stop: HashSet<&str> = [
        "what", "is", "the", "a", "an", "of", "in", "to", "for", "and", "or", "how", "many",
        "much", "does", "did", "was", "were", "are", "do", "which", "who", "where", "when", "why",
        "that", "this", "with", "from", "by", "it", "be", "as", "on", "at", "not", "but", "if",
        "so", "about", "than", "its", "has", "have", "had", "can",
    ]
    .into_iter()
    .collect();
    question
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty() && !stop.contains(w.as_str()))
        .collect()
}

fn extract_entity_mentions(question: &str) -> Vec<String> {
    question
        .split_whitespace()
        .filter(|w| {
            let t = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
            !t.is_empty() && (t.chars().next().is_some_and(|c| c.is_uppercase()) || t.contains('-'))
        })
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic_loop::types::MemoryFact;
    use std::collections::HashMap;

    struct FakeRetriever(Vec<MemoryFact>);
    impl MemoryRetriever for FakeRetriever {
        fn search(&self, _: &str, limit: usize) -> Vec<MemoryFact> {
            self.0.iter().take(limit).cloned().collect()
        }
        fn store_fact(&self, _: &str, _: &str, _: f64, _: &[String]) {}
    }

    fn mf(id: &str, ctx: &str, out: &str) -> MemoryFact {
        MemoryFact {
            id: id.into(),
            context: ctx.into(),
            outcome: out.into(),
            confidence: 1.0,
            metadata: HashMap::new(),
        }
    }

    fn qa_fact(id: &str) -> MemoryFact {
        let mut meta = HashMap::new();
        meta.insert("tags".into(), serde_json::json!(["q_and_a"]));
        MemoryFact {
            id: id.into(),
            context: "Question: X?".into(),
            outcome: "Answer: Y".into(),
            confidence: 0.7,
            metadata: meta,
        }
    }

    #[test]
    fn simple_intent_uses_simple_path() {
        let r = FakeRetriever(vec![mf("1", "ctx", "out")]);
        let i = DetectedIntent {
            intent: super::super::types::IntentType::SimpleRecall,
            ..Default::default()
        };
        let o = adaptive_retrieve("test", &i, &r, &SynthesisConfig::default());
        assert!(o.used_simple_path);
    }

    #[test]
    fn aggregation_intent() {
        let r = FakeRetriever(vec![mf("1", "ctx", "out")]);
        let i = DetectedIntent {
            intent: super::super::types::IntentType::MetaMemory,
            ..Default::default()
        };
        let o = adaptive_retrieve("list all", &i, &r, &SynthesisConfig::default());
        assert!(!o.used_simple_path && o.facts.len() == 1);
    }

    #[test]
    fn qa_echo_filtered() {
        assert!(is_qa_echo(&qa_fact("q1")));
        assert!(!is_qa_echo(&mf("f1", "Dogs", "are mammals")));
    }

    #[test]
    fn dedup_and_filter_works() {
        let facts = vec![
            mf("1", "a", "b"),
            mf("1", "a", "b"),
            qa_fact("q1"),
            mf("2", "c", "d"),
        ];
        let result = dedup_and_filter(facts);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn keywords_filter_stop_words() {
        let kws = extract_supplement_keywords("What is the total budget for Project Alpha?");
        assert!(kws.contains(&"total".to_string()) && kws.contains(&"budget".to_string()));
        assert!(!kws.contains(&"what".to_string()));
    }

    #[test]
    fn entity_mentions_finds_capitalised() {
        let entities = extract_entity_mentions("How are CVE-2024-001 and Alpha related?");
        assert!(entities.contains(&"CVE-2024-001".to_string()));
        assert!(entities.contains(&"Alpha".to_string()));
    }

    #[test]
    fn supplement_adds_new() {
        let r = FakeRetriever(vec![mf("new1", "budget", "100k")]);
        let mut existing = vec![mf("old1", "ctx", "out")];
        let cfg = SynthesisConfig {
            keyword_expansion_sparse_threshold: 50,
            ..Default::default()
        };
        supplement_keyword_expanded("total budget", &mut existing, &r, &cfg);
        assert!(existing.len() >= 2);
    }

    #[test]
    fn supplement_skipped_above_threshold() {
        let mut existing: Vec<MemoryFact> =
            (0..50).map(|i| mf(&format!("f{i}"), "c", "o")).collect();
        let len = existing.len();
        let cfg = SynthesisConfig {
            keyword_expansion_sparse_threshold: 30,
            ..Default::default()
        };
        supplement_keyword_expanded("test", &mut existing, &FakeRetriever(vec![]), &cfg);
        assert_eq!(existing.len(), len);
    }
}
