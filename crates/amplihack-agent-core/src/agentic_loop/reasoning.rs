//! Iterative reasoning: plan → search → evaluate → refine → answer.
//!
//! Ports the Python methods `reason_iteratively`, `_plan_retrieval`,
//! `_refine_retrieval`, `_evaluate_sufficiency`, and `_targeted_search`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use tracing::debug;

use crate::error::AgentError;

use super::json_parse::parse_json_response;
use super::loop_core::AgenticLoop;
use super::traits::{ActionExecutor, LlmClient, MemoryRetriever};
use super::types::{
    LlmMessage, MemoryFact, ReasoningStep, ReasoningTrace, RetrievalPlan,
    SufficiencyEvaluation,
};

impl<L, A, M> AgenticLoop<L, A, M>
where
    L: LlmClient,
    A: ActionExecutor,
    M: MemoryRetriever,
{
    /// Multi-step reasoning: plan, search, evaluate, refine, answer.
    ///
    /// Returns `(collected_facts, collected_nodes_as_facts, trace)`.
    pub async fn reason_iteratively(
        &self,
        question: &str,
        intent: &HashMap<String, Value>,
        max_steps: usize,
    ) -> Result<(Vec<HashMap<String, Value>>, Vec<MemoryFact>, ReasoningTrace), AgentError> {
        let mut collected_facts: Vec<HashMap<String, Value>> = Vec::new();
        let mut collected_nodes: Vec<MemoryFact> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut evaluation = SufficiencyEvaluation::default();

        let mut trace = ReasoningTrace {
            question: question.to_string(),
            intent: intent.clone(),
            ..Default::default()
        };

        let intent_type = intent
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or("simple_recall");

        let search_max_nodes: usize =
            if intent_type == "multi_source_synthesis" || intent_type == "temporal_comparison" {
                30
            } else {
                10
            };

        let mut total_queries: usize = 0;
        let mut last_step: usize = 0;
        let mut had_queries = false;

        for step in 0..max_steps {
            last_step = step;

            // Step 1: Plan or refine retrieval.
            let plan = if step == 0 {
                let p = self.plan_retrieval(question, intent).await;
                trace.steps.push(ReasoningStep::plan_or_refine(
                    "plan",
                    p.search_queries.clone(),
                    p.reasoning.clone(),
                ));
                p
            } else {
                let p = self
                    .refine_retrieval(question, &collected_facts, &evaluation)
                    .await;
                trace.steps.push(ReasoningStep::plan_or_refine(
                    "refine",
                    p.search_queries.clone(),
                    p.reasoning.clone(),
                ));
                p
            };

            if plan.search_queries.is_empty() {
                debug!("No search queries generated at step {step}, stopping");
                break;
            }
            had_queries = true;

            // Step 2: Targeted search.
            let mut new_facts_this_round: usize = 0;
            for query in &plan.search_queries {
                total_queries += 1;
                let (nodes, facts) =
                    self.targeted_search(query, &seen_ids, search_max_nodes);
                for (node, fact) in nodes.into_iter().zip(facts.into_iter()) {
                    if seen_ids.insert(node.id.clone()) {
                        collected_nodes.push(node);
                        collected_facts.push(fact);
                        new_facts_this_round += 1;
                    }
                }
            }

            trace.steps.push(ReasoningStep::search(
                plan.search_queries.clone(),
                new_facts_this_round,
            ));

            debug!(
                "Step {step}: {new_facts_this_round} new facts from {} queries",
                plan.search_queries.len()
            );

            // No new facts → stop early (unless first step).
            if new_facts_this_round == 0 && step > 0 {
                break;
            }

            // Step 3: Evaluate sufficiency.
            evaluation = self
                .evaluate_sufficiency(question, &collected_facts, intent)
                .await;

            trace.steps.push(ReasoningStep::evaluate(
                evaluation.sufficient,
                evaluation.confidence,
                &evaluation.missing,
            ));

            if evaluation.sufficient || evaluation.confidence > 0.8 {
                debug!(
                    "Sufficient at step {step} (confidence={:.2})",
                    evaluation.confidence
                );
                break;
            }
        }

        trace.total_facts_collected = collected_facts.len();
        trace.total_queries_executed = total_queries;
        trace.iterations = if had_queries {
            (last_step + 1).min(max_steps)
        } else {
            0
        };
        trace.final_confidence = evaluation.confidence;

        Ok((collected_facts, collected_nodes, trace))
    }

    // ------------------------------------------------------------------
    // _plan_retrieval
    // ------------------------------------------------------------------

    /// Plan what information to retrieve (one short LLM call).
    pub(crate) async fn plan_retrieval(
        &self,
        question: &str,
        intent: &HashMap<String, Value>,
    ) -> RetrievalPlan {
        let intent_type = intent
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or("simple_recall");

        let extra = match intent_type {
            "multi_source_synthesis" => concat!(
                "\n\nIMPORTANT: This question requires combining information from MULTIPLE sources.\n",
                "Strategy:\n",
                "1. Identify the DISTINCT topics/sources\n",
                "2. Generate at least ONE search query for EACH distinct topic\n",
                "3. Include one BROAD query using the main subject\n",
                "4. Include one query targeting COMPARISONS or RELATIONSHIPS\n\n",
                "You MUST generate at least 3 queries covering different aspects."
            ),
            "temporal_comparison" | "mathematical_computation" => concat!(
                "\n\nIMPORTANT: This question requires comparing data across TIME PERIODS.\n",
                "Strategy:\n",
                "1. Extract EVERY time period mentioned\n",
                "2. Generate a SEPARATE search query for EACH time period\n",
                "3. Each query MUST include the exact time marker\n",
                "4. Also include a query for the specific metric\n\n",
                "CRITICAL: Do NOT skip any time period."
            ),
            _ => "",
        };

        let prompt = format!(
            "Given this question, what specific information do I need to find in a knowledge base?\n\n\
             Question: {question}\n\
             Question type: {intent_type}\n\
             {extra}\n\n\
             Generate 2-5 SHORT, TARGETED search queries (keywords/phrases).\n\
             Each query should target ONE specific piece of information.\n\n\
             Return ONLY a JSON object:\n\
             {{\"search_queries\": [\"query1\", \"query2\", ...], \"reasoning\": \"brief explanation\"}}"
        );

        let messages = vec![
            LlmMessage::system(
                "You are a search planner. Generate targeted search queries. Return only JSON.",
            ),
            LlmMessage::user(prompt),
        ];

        if let Ok(response) = self.llm().completion(&messages, &self.model, 0.0).await
            && let Some(result) = parse_json_response(&response)
            && let Some(Value::Array(queries)) = result.get("search_queries")
        {
            let qs: Vec<String> = queries
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .take(5)
                .collect();
            let reasoning = result
                .get("reasoning")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            return RetrievalPlan {
                search_queries: qs,
                reasoning,
            };
        }

        // Fallback.
        RetrievalPlan {
            search_queries: vec![question.to_string()],
            reasoning: "fallback: using original question".into(),
        }
    }

    // ------------------------------------------------------------------
    // _refine_retrieval
    // ------------------------------------------------------------------

    /// Refine retrieval based on what is missing (one short LLM call).
    pub(crate) async fn refine_retrieval(
        &self,
        question: &str,
        collected_facts: &[HashMap<String, Value>],
        evaluation: &SufficiencyEvaluation,
    ) -> RetrievalPlan {
        let facts_summary: String = collected_facts
            .iter()
            .take(10)
            .map(|f| {
                let ctx = f
                    .get("context")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let outcome = f
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let short: String = outcome.chars().take(80).collect();
                format!("- [{ctx}] {short}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "I'm trying to answer: {question}\n\n\
             I already found these facts:\n{facts_summary}\n\n\
             What's missing: {}\n\n\
             Generate 2-3 NEW search queries targeting the MISSING information.\n\
             Use different keywords than before.\n\n\
             Return ONLY a JSON object:\n\
             {{\"search_queries\": [\"query1\", \"query2\"], \"reasoning\": \"what these queries target\"}}",
            evaluation.missing,
        );

        let messages = vec![
            LlmMessage::system(
                "You are a search planner. Generate targeted search queries for missing information. Return only JSON.",
            ),
            LlmMessage::user(prompt),
        ];

        if let Ok(response) = self.llm().completion(&messages, &self.model, 0.0).await
            && let Some(result) = parse_json_response(&response)
            && let Some(Value::Array(queries)) = result.get("search_queries")
        {
            let qs: Vec<String> = queries
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .take(3)
                .collect();
            let reasoning = result
                .get("reasoning")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            return RetrievalPlan {
                search_queries: qs,
                reasoning,
            };
        }

        // Fallback: use evaluation's refined queries.
        if !evaluation.refined_queries.is_empty() {
            return RetrievalPlan {
                search_queries: evaluation.refined_queries.iter().take(3).cloned().collect(),
                reasoning: "from evaluation suggestions".into(),
            };
        }

        RetrievalPlan::default()
    }

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
fn fact_to_map(fact: &MemoryFact) -> HashMap<String, Value> {
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
