//! Expert panel pattern — multi-expert independent reviews with vote aggregation.
//!
//! Native Rust port of `patterns/expert_panel.py`. The panel runs N expert
//! reviews in parallel via a shared [`ProcessRunner`], parses the structured
//! markdown response, then applies one of three aggregation strategies.

pub mod roles;
pub mod scoring;
pub mod synthesis;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::claude_process::ProcessRunner;
use crate::execution::run_parallel;
use crate::session::OrchestratorSession;

pub use synthesis::{
    aggregate_simple_majority, aggregate_unanimous, aggregate_weighted, generate_dissent_report,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoteChoice {
    Approve,
    Reject,
    Abstain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationMethod {
    SimpleMajority,
    Weighted,
    Unanimous,
}

#[derive(Debug, Clone)]
pub struct ExpertReview {
    pub expert_id: String,
    pub domain: String,
    pub analysis: String,
    pub strengths: Vec<String>,
    pub weaknesses: Vec<String>,
    pub domain_scores: HashMap<String, f32>,
    pub vote: VoteChoice,
    pub confidence: f32,
    pub vote_rationale: String,
    pub review_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct AggregatedDecision {
    pub decision: VoteChoice,
    pub confidence: f32,
    pub total_votes: usize,
    pub approve_votes: usize,
    pub reject_votes: usize,
    pub abstain_votes: usize,
    pub consensus_type: String,
    pub agreement_percentage: f32,
    pub dissenting_opinions: Vec<ExpertReview>,
    pub aggregation_method: String,
    pub quorum_met: bool,
}

#[derive(Debug, Clone)]
pub struct DissentReport {
    pub decision: VoteChoice,
    pub majority_count: usize,
    pub dissent_count: usize,
    pub majority_experts: Vec<String>,
    pub dissent_experts: Vec<String>,
    pub dissent_rationales: Vec<String>,
    pub concerns_raised: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExpertPanelResult {
    pub success: bool,
    pub decision: Option<AggregatedDecision>,
    pub dissent_report: Option<DissentReport>,
    pub reviews: Vec<ExpertReview>,
    pub session_id: String,
}

fn parse_vote(text: &str) -> VoteChoice {
    let body = scoring::extract_section(text, "Vote");
    match body.trim().to_uppercase().as_str() {
        "APPROVE" => VoteChoice::Approve,
        "REJECT" => VoteChoice::Reject,
        _ => VoteChoice::Abstain,
    }
}

fn parse_confidence(text: &str) -> f32 {
    let body = scoring::extract_section(text, "Confidence");
    body.trim()
        .parse::<f32>()
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(0.5)
}

fn parse_review(
    output: &str,
    expert: &roles::Expert,
    expert_id: String,
    duration: Duration,
) -> ExpertReview {
    ExpertReview {
        expert_id,
        domain: expert.domain.to_string(),
        analysis: scoring::extract_section(output, "Analysis"),
        strengths: scoring::extract_list_items(output, "Strengths"),
        weaknesses: scoring::extract_list_items(output, "Weaknesses"),
        domain_scores: scoring::extract_scores(output, "Domain Scores"),
        vote: parse_vote(output),
        confidence: parse_confidence(output),
        vote_rationale: scoring::extract_section(output, "Vote Rationale"),
        review_duration: duration,
    }
}

/// Run an expert panel review.
#[allow(clippy::too_many_arguments)]
pub async fn run_expert_panel(
    solution: String,
    experts: Option<Vec<roles::Expert>>,
    aggregation_method: AggregationMethod,
    quorum: usize,
    model: Option<String>,
    working_dir: Option<PathBuf>,
    timeout: Option<Duration>,
    runner: Arc<dyn ProcessRunner>,
) -> ExpertPanelResult {
    let experts: Vec<roles::Expert> = experts.unwrap_or_else(|| roles::DEFAULT_EXPERTS.to_vec());
    let working_dir = working_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let mut builder = OrchestratorSession::builder()
        .pattern_name("expert-panel")
        .working_dir(working_dir);
    if let Some(m) = model.clone() {
        builder = builder.model(m);
    }
    let mut session = builder.runner(runner).build().expect("session build");

    session.log_info(&format!(
        "Starting Expert Panel Review with {} experts using {:?} aggregation",
        experts.len(),
        aggregation_method
    ));

    let mut processes = Vec::new();
    let mut metadata = Vec::new();
    for expert in &experts {
        let prompt = roles::build_review_prompt(&solution, expert, experts.len());
        let pid = format!("expert_{}", expert.domain);
        let process = session
            .create_process(&prompt, Some(&pid), model.as_deref(), timeout)
            .expect("create_process");
        processes.push(process);
        metadata.push((*expert, pid));
    }

    let start = Instant::now();
    let results = run_parallel(processes, None).await;
    let panel_duration = start.elapsed();
    session.log_info(&format!("Panel review completed in {panel_duration:?}"));

    let mut reviews = Vec::new();
    for (result, (expert, expert_id)) in results.into_iter().zip(metadata) {
        if !result.is_success() {
            session.log_warn(&format!(
                "Expert {} failed: {}",
                expert.domain, result.stderr
            ));
            reviews.push(ExpertReview {
                expert_id,
                domain: expert.domain.to_string(),
                analysis: format!("Review failed: {}", result.stderr),
                strengths: vec![],
                weaknesses: vec!["Review unavailable".into()],
                domain_scores: HashMap::new(),
                vote: VoteChoice::Abstain,
                confidence: 0.0,
                vote_rationale: "Review process failed".into(),
                review_duration: result.duration,
            });
            continue;
        }
        reviews.push(parse_review(
            &result.output,
            &expert,
            expert_id,
            result.duration,
        ));
    }
    let decision = match aggregation_method {
        AggregationMethod::SimpleMajority => aggregate_simple_majority(&reviews, quorum),
        AggregationMethod::Weighted => aggregate_weighted(&reviews, quorum),
        AggregationMethod::Unanimous => aggregate_unanimous(&reviews, quorum),
    };
    let dissent_report = generate_dissent_report(&decision);
    let session_id = session.session_id().to_string();
    session.log_info(&format!(
        "Decision: {:?} (consensus={}, quorum_met={})",
        decision.decision, decision.consensus_type, decision.quorum_met
    ));

    ExpertPanelResult {
        success: decision.quorum_met,
        decision: Some(decision),
        dissent_report,
        reviews,
        session_id,
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn parse_vote_handles_all_choices() {
        assert_eq!(parse_vote("## Vote\nAPPROVE\n"), VoteChoice::Approve);
        assert_eq!(parse_vote("## Vote\nREJECT\n"), VoteChoice::Reject);
        assert_eq!(parse_vote("## Vote\nABSTAIN\n"), VoteChoice::Abstain);
        assert_eq!(parse_vote("## Vote\nbogus\n"), VoteChoice::Abstain);
    }
    #[test]
    fn parse_confidence_clamps_and_defaults() {
        assert_eq!(parse_confidence("## Confidence\n0.5\n"), 0.5);
        assert_eq!(parse_confidence("## Confidence\n2.0\n"), 1.0);
        assert_eq!(parse_confidence("## Confidence\n-1\n"), 0.0);
        assert_eq!(parse_confidence("## Confidence\nbogus\n"), 0.5);
    }
}
