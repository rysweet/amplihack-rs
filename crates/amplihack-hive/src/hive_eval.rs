//! Hive evaluation — event-driven query/response evaluation.
//!
//! Matches Python `amplihack/workloads/hive/_eval.py`:
//! - Publishes HIVE_QUERY events
//! - Collects HIVE_QUERY_RESPONSE answers
//! - Builds eval question sets
//! - Aggregates results per query

use crate::error::{HiveError, Result};
use crate::event_bus::EventBus;
use crate::hive_events::{HIVE_QUERY_RESPONSE, make_query_event};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Eval handler ID for subscribing to response events.
const EVAL_HANDLER: &str = "__hive_eval_collector";

/// Configuration for a hive evaluation round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveEvalConfig {
    pub questions: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_min_responses")]
    pub min_responses_per_query: usize,
}

fn default_timeout() -> u64 {
    60
}

fn default_min_responses() -> usize {
    1
}

impl HiveEvalConfig {
    pub fn new(questions: Vec<String>) -> Self {
        Self {
            questions,
            timeout_seconds: default_timeout(),
            min_responses_per_query: default_min_responses(),
        }
    }

    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    pub fn with_min_responses(mut self, min: usize) -> Self {
        self.min_responses_per_query = min;
        self
    }
}

/// An individual agent's response to a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAnswer {
    pub agent_id: String,
    pub answer: String,
    pub confidence: f64,
}

/// Result of a single query across all responding agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub query_id: String,
    pub question: String,
    pub answers: Vec<AgentAnswer>,
}

impl QueryResult {
    pub fn best_answer(&self) -> Option<&AgentAnswer> {
        self.answers.iter().max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn average_confidence(&self) -> f64 {
        if self.answers.is_empty() {
            return 0.0;
        }
        self.answers.iter().map(|a| a.confidence).sum::<f64>() / self.answers.len() as f64
    }

    pub fn response_count(&self) -> usize {
        self.answers.len()
    }
}

/// Aggregate result of a hive evaluation round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveEvalResult {
    pub query_results: Vec<QueryResult>,
    pub total_queries: usize,
    pub total_responses: usize,
    pub average_confidence: f64,
}

impl HiveEvalResult {
    pub fn from_results(results: Vec<QueryResult>) -> Self {
        let total_queries = results.len();
        let total_responses: usize = results.iter().map(|r| r.answers.len()).sum();
        let average_confidence = if total_responses == 0 {
            0.0
        } else {
            results
                .iter()
                .flat_map(|r| r.answers.iter().map(|a| a.confidence))
                .sum::<f64>()
                / total_responses as f64
        };
        Self {
            query_results: results,
            total_queries,
            total_responses,
            average_confidence,
        }
    }
}

/// Publish evaluation queries and collect responses.
///
/// In the sync implementation, this publishes queries to the bus.
/// Real response collection happens asynchronously via the event bus
/// subscription; this function provides the query-side orchestration.
pub fn run_eval(bus: &mut dyn EventBus, config: &HiveEvalConfig) -> Result<HiveEvalResult> {
    if config.questions.is_empty() {
        return Err(HiveError::Workload(
            "No evaluation questions provided".into(),
        ));
    }

    // Subscribe to response topic
    bus.subscribe(EVAL_HANDLER, Some(&[HIVE_QUERY_RESPONSE]))
        .map_err(|e| HiveError::EventBus(format!("Failed to subscribe for eval: {e}")))?;

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(config.timeout_seconds);

    info!(
        questions = config.questions.len(),
        timeout = config.timeout_seconds,
        "Starting hive evaluation"
    );

    let mut query_results = Vec::with_capacity(config.questions.len());

    for question in &config.questions {
        if std::time::Instant::now() >= deadline {
            warn!(
                remaining = config.questions.len() - query_results.len(),
                "Eval timeout reached, skipping remaining questions"
            );
            break;
        }

        let result = eval_single_question(bus, question, config.min_responses_per_query);
        query_results.push(result);
    }

    // Unsubscribe — log but don't fail the whole eval on cleanup error
    if let Err(e) = bus.unsubscribe(EVAL_HANDLER) {
        warn!(error = %e, "Failed to unsubscribe eval handler");
    }

    let result = HiveEvalResult::from_results(query_results);
    info!(
        queries = result.total_queries,
        responses = result.total_responses,
        confidence = format!("{:.2}", result.average_confidence),
        "Hive evaluation complete"
    );

    Ok(result)
}

/// Evaluate a single question: publish the query and collect responses.
fn eval_single_question(
    bus: &mut dyn EventBus,
    question: &str,
    min_responses: usize,
) -> QueryResult {
    let (query_id, event) = match make_query_event(question) {
        Ok(pair) => pair,
        Err(e) => {
            warn!(error = %e, "Failed to create query event");
            return QueryResult {
                query_id: String::new(),
                question: question.to_string(),
                answers: vec![],
            };
        }
    };

    debug!(query_id = %query_id, question = %question, "Publishing query");

    if let Err(e) = bus.publish(event) {
        warn!(query_id = %query_id, error = %e, "Failed to publish query");
        return QueryResult {
            query_id,
            question: question.to_string(),
            answers: vec![],
        };
    }

    let answers = collect_responses(bus, &query_id);

    if answers.len() < min_responses {
        warn!(
            query_id = %query_id,
            got = answers.len(),
            min = min_responses,
            "Fewer responses than min_responses_per_query"
        );
    }

    QueryResult {
        query_id,
        question: question.to_string(),
        answers,
    }
}

/// Collect responses for a specific query from the event bus.
fn collect_responses(bus: &mut dyn EventBus, query_id: &str) -> Vec<AgentAnswer> {
    let events = bus.poll(EVAL_HANDLER).unwrap_or_else(|e| {
        warn!(error = %e, "poll failed, falling back to empty");
        Vec::new()
    });
    let mut answers = Vec::new();

    for event in events {
        match serde_json::from_value::<crate::workload::HiveEvent>(event.payload.clone()) {
            Ok(crate::workload::HiveEvent::QueryResponse {
                query_id: qid,
                answer,
                confidence,
            }) if qid == query_id => {
                answers.push(AgentAnswer {
                    agent_id: event.source_id,
                    answer,
                    confidence,
                });
            }
            _ => {
                // Unmatched events are dropped; re-publishing would duplicate
                // them to all subscribers. Log a warning instead.
                warn!(
                    event_topic = %event.topic,
                    "Dropping unmatched event during eval collection"
                );
            }
        }
    }

    answers
}

/// Build a default set of evaluation questions.
pub fn build_default_eval_questions() -> Vec<String> {
    vec![
        "What are the key benefits of Rust's ownership system?".into(),
        "How does the borrow checker prevent data races?".into(),
        "What is the difference between &str and String?".into(),
        "Explain the purpose of the ? operator in error handling.".into(),
        "What are traits and how do they enable polymorphism?".into(),
    ]
}

#[cfg(test)]
#[path = "tests/hive_eval_tests.rs"]
mod tests;
