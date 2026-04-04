//! Hive haymaker CLI commands: `hive feed` and `hive eval`.
//!
//! Ported from `amplihack/cli/hive_haymaker.py`.
//!
//! Provides two subcommand handlers that wrap the `amplihack-hive` crate's
//! feed and evaluation logic. The actual Service Bus transport is not
//! available in the Rust runtime; commands operate against a local
//! [`LocalEventBus`] for testing/demo or log a stub message for production.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use amplihack_hive::event_bus::LocalEventBus;
use amplihack_hive::feed::{FeedConfig, FeedResult, build_default_content_pool};
use amplihack_hive::hive_eval::{HiveEvalConfig, HiveEvalResult, build_default_eval_questions};

// ---------------------------------------------------------------------------
// Feed
// ---------------------------------------------------------------------------

/// Arguments for `haymaker hive feed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveFeedArgs {
    /// Hive deployment ID.
    pub deployment_id: String,
    /// Number of LEARN_CONTENT events to send.
    #[serde(default = "default_turns")]
    pub turns: u32,
    /// Override Service Bus topic name.
    pub topic: Option<String>,
    /// Service Bus connection string (not used in stub mode).
    #[serde(default)]
    pub sb_conn_str: String,
}

fn default_turns() -> u32 {
    100
}

/// Arguments for `haymaker hive eval`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveEvalArgs {
    /// Hive deployment ID.
    pub deployment_id: String,
    /// Number of question rounds.
    #[serde(default = "default_repeats")]
    pub repeats: u32,
    /// Number of AGENT_READY events to wait for (0 = skip).
    #[serde(default)]
    pub wait_for_ready: u32,
    /// Max seconds to wait for agents to become ready.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Override Service Bus topic name.
    pub topic: Option<String>,
    /// Service Bus connection string (not used in stub mode).
    #[serde(default)]
    pub sb_conn_str: String,
    /// Output format.
    #[serde(default = "default_output_format")]
    pub output: OutputFormat,
}

fn default_repeats() -> u32 {
    3
}

fn default_timeout() -> u64 {
    600
}

/// Output format for eval results.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

fn default_output_format() -> OutputFormat {
    OutputFormat::Text
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Run the `hive feed` command.
///
/// Publishes `turns` LEARN_CONTENT events followed by a FEED_COMPLETE
/// sentinel. In stub mode (no Service Bus transport), operates against a
/// local in-memory event bus.
pub fn run_hive_feed(args: &HiveFeedArgs) -> Result<FeedResult> {
    let resolved_topic = args
        .topic
        .clone()
        .or_else(|| std::env::var("AMPLIHACK_TOPIC_NAME").ok())
        .unwrap_or_else(|| "hive-graph".to_string());

    info!(
        deployment_id = %args.deployment_id,
        turns = args.turns,
        topic = %resolved_topic,
        "Feeding content into hive"
    );

    if !args.sb_conn_str.is_empty() {
        warn!("Service Bus transport is not yet implemented in Rust; using local event bus");
    }

    let pool = build_default_content_pool();
    let items: Vec<String> = pool.into_iter().cycle().take(args.turns as usize).collect();

    let config = FeedConfig::new(&args.deployment_id, items);
    let mut bus = LocalEventBus::new();

    let result = amplihack_hive::feed::run_feed(&mut bus, &config).context("hive feed failed")?;

    info!(
        items_published = result.items_published,
        errors = result.errors.len(),
        "Feed complete"
    );

    Ok(result)
}

/// Run the `hive eval` command.
///
/// Optionally waits for AGENT_READY events, then runs `repeats` question
/// rounds. In stub mode, operates against a local in-memory event bus.
pub fn run_hive_eval(args: &HiveEvalArgs) -> Result<HiveEvalResult> {
    let resolved_topic = args
        .topic
        .clone()
        .or_else(|| std::env::var("AMPLIHACK_TOPIC_NAME").ok())
        .unwrap_or_else(|| "hive-graph".to_string());

    if args.wait_for_ready > 0 {
        info!(
            wait_for_ready = args.wait_for_ready,
            timeout = args.timeout,
            "Waiting for agents to signal AGENT_READY (stub — skipping)"
        );
        warn!("AGENT_READY wait is stubbed in Rust; proceeding immediately");
    }

    info!(
        deployment_id = %args.deployment_id,
        repeats = args.repeats,
        topic = %resolved_topic,
        "Running eval rounds"
    );

    if !args.sb_conn_str.is_empty() {
        warn!("Service Bus transport is not yet implemented in Rust; using local event bus");
    }

    let questions = build_default_eval_questions();
    let eval_questions: Vec<String> = questions
        .into_iter()
        .cycle()
        .take(args.repeats as usize)
        .collect();

    let config = HiveEvalConfig::new(eval_questions).with_timeout(args.timeout);
    let mut bus = LocalEventBus::new();

    let result = amplihack_hive::hive_eval::run_eval(&mut bus, &config)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("hive eval failed")?;

    info!(
        total_queries = result.total_queries,
        total_responses = result.total_responses,
        avg_confidence = format!("{:.2}", result.average_confidence),
        "Eval complete"
    );

    Ok(result)
}

/// Format eval results for human-readable text output.
pub fn format_eval_text(result: &HiveEvalResult) -> String {
    let mut out = String::new();
    for (i, qr) in result.query_results.iter().enumerate() {
        out.push_str(&format!("\n--- Round {} ---\n", i + 1));
        out.push_str(&format!("Q: {}\n", qr.question));
        if qr.answers.is_empty() {
            out.push_str("  (no responses received)\n");
        } else {
            for ans in &qr.answers {
                out.push_str(&format!("  [{}] {}\n", ans.agent_id, ans.answer));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- HiveFeedArgs -------------------------------------------------------

    #[test]
    fn feed_args_defaults() {
        let args: HiveFeedArgs = serde_json::from_str(r#"{"deployment_id": "d1"}"#).unwrap();
        assert_eq!(args.turns, 100);
        assert!(args.topic.is_none());
        assert!(args.sb_conn_str.is_empty());
    }

    #[test]
    fn feed_args_custom() {
        let args: HiveFeedArgs =
            serde_json::from_str(r#"{"deployment_id":"d2","turns":10,"topic":"custom"}"#).unwrap();
        assert_eq!(args.turns, 10);
        assert_eq!(args.topic.as_deref(), Some("custom"));
    }

    #[test]
    fn run_hive_feed_succeeds() {
        let args = HiveFeedArgs {
            deployment_id: "test-deploy".into(),
            turns: 5,
            topic: None,
            sb_conn_str: String::new(),
        };
        let result = run_hive_feed(&args).unwrap();
        assert_eq!(result.items_published, 5);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn run_hive_feed_zero_turns() {
        let args = HiveFeedArgs {
            deployment_id: "test-zero".into(),
            turns: 0,
            topic: None,
            sb_conn_str: String::new(),
        };
        let result = run_hive_feed(&args).unwrap();
        assert_eq!(result.items_published, 0);
    }

    // -- HiveEvalArgs -------------------------------------------------------

    #[test]
    fn eval_args_defaults() {
        let args: HiveEvalArgs = serde_json::from_str(r#"{"deployment_id": "d1"}"#).unwrap();
        assert_eq!(args.repeats, 3);
        assert_eq!(args.wait_for_ready, 0);
        assert_eq!(args.timeout, 600);
        assert_eq!(args.output, OutputFormat::Text);
    }

    #[test]
    fn eval_args_json_output() {
        let args: HiveEvalArgs =
            serde_json::from_str(r#"{"deployment_id":"d1","output":"json"}"#).unwrap();
        assert_eq!(args.output, OutputFormat::Json);
    }

    #[test]
    fn run_hive_eval_succeeds() {
        let args = HiveEvalArgs {
            deployment_id: "test-eval".into(),
            repeats: 2,
            wait_for_ready: 0,
            timeout: 10,
            topic: None,
            sb_conn_str: String::new(),
            output: OutputFormat::Text,
        };
        let result = run_hive_eval(&args).unwrap();
        assert_eq!(result.total_queries, 2);
    }

    #[test]
    fn run_hive_eval_with_ready_wait() {
        let args = HiveEvalArgs {
            deployment_id: "test-wait".into(),
            repeats: 1,
            wait_for_ready: 5,
            timeout: 10,
            topic: Some("custom-topic".into()),
            sb_conn_str: String::new(),
            output: OutputFormat::Json,
        };
        // Should succeed — wait is stubbed.
        let result = run_hive_eval(&args).unwrap();
        assert_eq!(result.total_queries, 1);
    }

    #[test]
    fn format_eval_text_empty_answers() {
        let result = HiveEvalResult {
            query_results: vec![amplihack_hive::hive_eval::QueryResult {
                query_id: "q1".into(),
                question: "What is Rust?".into(),
                answers: vec![],
            }],
            total_queries: 1,
            total_responses: 0,
            average_confidence: 0.0,
        };
        let text = format_eval_text(&result);
        assert!(text.contains("Round 1"));
        assert!(text.contains("What is Rust?"));
        assert!(text.contains("no responses received"));
    }

    #[test]
    fn output_format_serde_roundtrip() {
        let json = serde_json::to_string(&OutputFormat::Json).unwrap();
        let restored: OutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, OutputFormat::Json);
    }
}
