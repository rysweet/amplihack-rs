//! Sufficiency evaluation and targeted search for the reasoning pipeline.
//!
//! Split from `reasoning.rs` to keep modules under 400 lines.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::json_parse::parse_json_response;
use super::loop_core::AgenticLoop;
use super::traits::{ActionExecutor, LlmClient, MemoryRetriever};
use super::types::{LlmMessage, MemoryFact, SufficiencyEvaluation};

impl<L, A, M> AgenticLoop<L, A, M>
where
    L: LlmClient,
    A: ActionExecutor,
    M: MemoryRetriever,
{
    // ------------------------------------------------------------------
    // _evaluate_sufficiency
    // ------------------------------------------------------------------

    /// Evaluate if collected facts are sufficient (one short LLM call).
    pub(crate) async fn evaluate_sufficiency(
        &self,
        question: &str,
        collected_facts: &[HashMap<String, Value>],
        intent: &HashMap<String, Value>,
    ) -> SufficiencyEvaluation {
        if collected_facts.is_empty() {
            return SufficiencyEvaluation {
                sufficient: false,
                missing: "No facts found yet".into(),
                confidence: 0.0,
                refined_queries: vec![question.to_string()],
            };
        }

        let facts_summary: String = collected_facts
            .iter()
            .take(15)
            .map(|f| {
                let ctx = f.get("context").and_then(Value::as_str).unwrap_or("?");
                let outcome = f.get("outcome").and_then(Value::as_str).unwrap_or("");
                let short: String = outcome.chars().take(100).collect();
                format!("- [{ctx}] {short}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        let intent_type = intent
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or("simple_recall");

        let prompt = format!(
            "Can I answer this question with these facts?\n\n\
             Question: {question}\n\
             Question type: {intent_type}\n\n\
             Available facts:\n{facts_summary}\n\n\
             Evaluate:\n\
             1. Do I have ALL the specific data points needed?\n\
             2. What is STILL MISSING (if anything)?\n\
             3. Confidence I can answer correctly (0.0-1.0)?\n\n\
             Return ONLY a JSON object:\n\
             {{\"sufficient\": true, \"missing\": \"\", \"confidence\": 0.8, \"refined_queries\": []}}"
        );

        let messages = vec![
            LlmMessage::system(
                "You are a fact sufficiency evaluator. Be strict: if key data points are missing, say so. Return only JSON.",
            ),
            LlmMessage::user(prompt),
        ];

        if let Ok(response) = self.llm().completion(&messages, &self.model, 0.0).await
            && let Some(result) = parse_json_response(&response)
        {
            return SufficiencyEvaluation {
                sufficient: result
                    .get("sufficient")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                missing: result
                    .get("missing")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                confidence: result
                    .get("confidence")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5),
                refined_queries: result
                    .get("refined_queries")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            };
        }

        // Conservative fallback.
        let n = collected_facts.len();
        SufficiencyEvaluation {
            sufficient: n >= 5,
            missing: if n >= 5 {
                String::new()
            } else {
                "unable to evaluate".into()
            },
            confidence: if n >= 5 { 0.6 } else { 0.3 },
            refined_queries: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // _targeted_search
    // ------------------------------------------------------------------

    /// Run a targeted search against memory.
    ///
    /// Returns `(nodes, facts)` — parallel vectors.
    pub(crate) fn targeted_search(
        &self,
        query: &str,
        seen_ids: &HashSet<String>,
        max_nodes: usize,
    ) -> (Vec<MemoryFact>, Vec<HashMap<String, Value>>) {
        let mut nodes = Vec::new();
        let mut facts = Vec::new();

        // Try facade first (richer API), then fallback to retriever.
        if let Some(facade) = self.facade() {
            let retrieved = facade.retrieve_facts(query, max_nodes);
            for node in retrieved {
                if !seen_ids.contains(&node.id) {
                    let fact_map = fact_to_map(&node);
                    facts.push(fact_map);
                    nodes.push(node);
                }
            }
        } else {
            let results = self.retriever().search(query, max_nodes);
            for r in results {
                if !seen_ids.contains(&r.id) {
                    let fact_map = fact_to_map(&r);
                    facts.push(fact_map);
                    nodes.push(r);
                }
            }
        }

        (nodes, facts)
    }
}

/// Convert a [`MemoryFact`] into a generic `HashMap<String, Value>`.
pub(crate) fn fact_to_map(fact: &MemoryFact) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("context".into(), Value::String(fact.context.clone()));
    m.insert("outcome".into(), Value::String(fact.outcome.clone()));
    m.insert(
        "confidence".into(),
        serde_json::to_value(fact.confidence).unwrap_or(Value::Null),
    );
    if !fact.metadata.is_empty() {
        m.insert(
            "metadata".into(),
            serde_json::to_value(&fact.metadata).unwrap_or(Value::Null),
        );
    }
    m
}
