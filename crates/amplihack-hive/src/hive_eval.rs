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
    bus.subscribe(HIVE_QUERY_RESPONSE, EVAL_HANDLER)
        .map_err(|e| HiveError::EventBus(format!("Failed to subscribe for eval: {e}")))?;

    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(config.timeout_seconds);

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

        let (query_id, event) = make_query_event(question)?;
        debug!(query_id = %query_id, question = %question, "Publishing query");

        if let Err(e) = bus.publish(event) {
            warn!(query_id = %query_id, error = %e, "Failed to publish query");
            query_results.push(QueryResult {
                query_id,
                question: question.clone(),
                answers: vec![],
            });
            continue;
        }

        // Collect responses from the bus for this query
        let answers = collect_responses(bus, &query_id);

        if answers.len() < config.min_responses_per_query {
            warn!(
                query_id = %query_id,
                got = answers.len(),
                min = config.min_responses_per_query,
                "Fewer responses than min_responses_per_query"
            );
        }

        query_results.push(QueryResult {
            query_id,
            question: question.clone(),
            answers,
        });
    }

    // Unsubscribe — log but don't fail the whole eval on cleanup error
    if let Err(e) = bus.unsubscribe(HIVE_QUERY_RESPONSE, EVAL_HANDLER) {
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

/// Collect responses for a specific query from the event bus.
fn collect_responses(bus: &mut dyn EventBus, query_id: &str) -> Vec<AgentAnswer> {
    let events = bus.drain_events(EVAL_HANDLER).unwrap_or_else(|e| {
        warn!(error = %e, "drain_events failed, falling back to empty");
        Vec::new()
    });
    let mut answers = Vec::new();

    for event in events {
        if let Ok(crate::workload::HiveEvent::QueryResponse {
            query_id: qid,
            answer,
            confidence,
        }) = serde_json::from_value::<crate::workload::HiveEvent>(event.payload)
            && qid == query_id
        {
            answers.push(AgentAnswer {
                agent_id: event.source_id,
                answer,
                confidence,
            });
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
mod tests {
    use super::*;
    use crate::event_bus::LocalEventBus;

    #[test]
    fn eval_config_defaults() {
        let config = HiveEvalConfig::new(vec!["q1".into()]);
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.min_responses_per_query, 1);
    }

    #[test]
    fn eval_config_builder() {
        let config = HiveEvalConfig::new(vec!["q1".into()])
            .with_timeout(120)
            .with_min_responses(3);
        assert_eq!(config.timeout_seconds, 120);
        assert_eq!(config.min_responses_per_query, 3);
    }

    #[test]
    fn run_eval_publishes_queries() {
        let mut bus = LocalEventBus::new();
        let config = HiveEvalConfig::new(vec!["Question 1?".into(), "Question 2?".into()]);
        let result = run_eval(&mut bus, &config).unwrap();
        assert_eq!(result.total_queries, 2);
    }

    #[test]
    fn run_eval_rejects_empty_questions() {
        let mut bus = LocalEventBus::new();
        let config = HiveEvalConfig::new(vec![]);
        assert!(run_eval(&mut bus, &config).is_err());
    }

    #[test]
    fn query_result_best_answer() {
        let result = QueryResult {
            query_id: "q1".into(),
            question: "test".into(),
            answers: vec![
                AgentAnswer {
                    agent_id: "a1".into(),
                    answer: "low".into(),
                    confidence: 0.3,
                },
                AgentAnswer {
                    agent_id: "a2".into(),
                    answer: "high".into(),
                    confidence: 0.9,
                },
            ],
        };
        let best = result.best_answer().unwrap();
        assert_eq!(best.agent_id, "a2");
        assert!((best.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn query_result_average_confidence() {
        let result = QueryResult {
            query_id: "q1".into(),
            question: "test".into(),
            answers: vec![
                AgentAnswer {
                    agent_id: "a1".into(),
                    answer: "a".into(),
                    confidence: 0.4,
                },
                AgentAnswer {
                    agent_id: "a2".into(),
                    answer: "b".into(),
                    confidence: 0.8,
                },
            ],
        };
        assert!((result.average_confidence() - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn query_result_empty() {
        let result = QueryResult {
            query_id: "q1".into(),
            question: "test".into(),
            answers: vec![],
        };
        assert!(result.best_answer().is_none());
        assert_eq!(result.average_confidence(), 0.0);
        assert_eq!(result.response_count(), 0);
    }

    #[test]
    fn hive_eval_result_aggregation() {
        let results = vec![
            QueryResult {
                query_id: "q1".into(),
                question: "a".into(),
                answers: vec![AgentAnswer {
                    agent_id: "a1".into(),
                    answer: "x".into(),
                    confidence: 0.8,
                }],
            },
            QueryResult {
                query_id: "q2".into(),
                question: "b".into(),
                answers: vec![],
            },
        ];
        let eval = HiveEvalResult::from_results(results);
        assert_eq!(eval.total_queries, 2);
        assert_eq!(eval.total_responses, 1);
    }

    #[test]
    fn default_eval_questions_non_empty() {
        let questions = build_default_eval_questions();
        assert!(!questions.is_empty());
        assert!(questions.len() >= 3);
    }

    #[test]
    fn hive_eval_config_serde() {
        let config = HiveEvalConfig::new(vec!["q".into()]);
        let json = serde_json::to_string(&config).unwrap();
        let restored: HiveEvalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.questions.len(), 1);
    }
}
