//! Vote aggregation strategies for the expert panel.
//!
//! Native Rust port of the `aggregate_*` functions and `generate_dissent_report`
//! from `patterns/expert_panel.py`. Mirrors the conservative tie-breaking
//! semantics (ties default to REJECT).

use crate::patterns::expert_panel::{AggregatedDecision, DissentReport, ExpertReview, VoteChoice};

fn count_votes(reviews: &[ExpertReview]) -> (usize, usize, usize) {
    let mut a = 0;
    let mut r = 0;
    let mut ab = 0;
    for review in reviews {
        match review.vote {
            VoteChoice::Approve => a += 1,
            VoteChoice::Reject => r += 1,
            VoteChoice::Abstain => ab += 1,
        }
    }
    (a, r, ab)
}

fn classify_consensus(pct: f32) -> &'static str {
    if (pct - 100.0).abs() < f32::EPSILON {
        "unanimous"
    } else if pct >= 75.0 {
        "strong_majority"
    } else if pct > 50.0 {
        "simple_majority"
    } else {
        "split"
    }
}

/// Simple-majority aggregation. Ties default to REJECT (conservative).
pub fn aggregate_simple_majority(reviews: &[ExpertReview], quorum: usize) -> AggregatedDecision {
    let (approve, reject, abstain) = count_votes(reviews);
    let total = reviews.len();
    let non_abstain = approve + reject;
    let quorum_met = non_abstain >= quorum;

    let (decision, majority_count, dissenting): (VoteChoice, usize, Vec<ExpertReview>) =
        if approve > reject {
            (
                VoteChoice::Approve,
                approve,
                reviews
                    .iter()
                    .filter(|r| r.vote == VoteChoice::Reject)
                    .cloned()
                    .collect(),
            )
        } else if reject > approve {
            (
                VoteChoice::Reject,
                reject,
                reviews
                    .iter()
                    .filter(|r| r.vote == VoteChoice::Approve)
                    .cloned()
                    .collect(),
            )
        } else {
            (
                VoteChoice::Reject,
                reject,
                reviews
                    .iter()
                    .filter(|r| r.vote == VoteChoice::Approve)
                    .cloned()
                    .collect(),
            )
        };

    let agreement_pct = if non_abstain > 0 {
        (majority_count as f32 / non_abstain as f32) * 100.0
    } else {
        0.0
    };
    let consensus_type = classify_consensus(agreement_pct).to_string();

    let relevant: Vec<&ExpertReview> = reviews.iter().filter(|r| r.vote == decision).collect();
    let avg_confidence = if relevant.is_empty() {
        0.5
    } else {
        let sum: f32 = relevant.iter().map(|r| r.confidence).sum();
        sum / relevant.len() as f32
    };

    AggregatedDecision {
        decision,
        confidence: avg_confidence,
        total_votes: total,
        approve_votes: approve,
        reject_votes: reject,
        abstain_votes: abstain,
        consensus_type,
        agreement_percentage: agreement_pct,
        dissenting_opinions: dissenting,
        aggregation_method: "simple_majority".to_string(),
        quorum_met,
    }
}

/// Confidence-weighted aggregation. Ties default to REJECT.
pub fn aggregate_weighted(reviews: &[ExpertReview], quorum: usize) -> AggregatedDecision {
    let (approve, reject, abstain) = count_votes(reviews);
    let total = reviews.len();
    let non_abstain = approve + reject;
    let quorum_met = non_abstain >= quorum;

    let approve_weight: f32 = reviews
        .iter()
        .filter(|r| r.vote == VoteChoice::Approve)
        .map(|r| r.confidence)
        .sum();
    let reject_weight: f32 = reviews
        .iter()
        .filter(|r| r.vote == VoteChoice::Reject)
        .map(|r| r.confidence)
        .sum();

    let (decision, majority_weight, dissenting): (VoteChoice, f32, Vec<ExpertReview>) =
        if approve_weight > reject_weight {
            (
                VoteChoice::Approve,
                approve_weight,
                reviews
                    .iter()
                    .filter(|r| r.vote == VoteChoice::Reject)
                    .cloned()
                    .collect(),
            )
        } else if reject_weight > approve_weight {
            (
                VoteChoice::Reject,
                reject_weight,
                reviews
                    .iter()
                    .filter(|r| r.vote == VoteChoice::Approve)
                    .cloned()
                    .collect(),
            )
        } else {
            (
                VoteChoice::Reject,
                reject_weight,
                reviews
                    .iter()
                    .filter(|r| r.vote == VoteChoice::Approve)
                    .cloned()
                    .collect(),
            )
        };

    let total_weight = approve_weight + reject_weight;
    let agreement_pct = if total_weight > 0.0 {
        (majority_weight / total_weight) * 100.0
    } else {
        0.0
    };
    let consensus_type = classify_consensus(agreement_pct).to_string();
    let denom = non_abstain.max(1) as f32;
    let confidence = (majority_weight / denom).min(1.0);

    AggregatedDecision {
        decision,
        confidence,
        total_votes: total,
        approve_votes: approve,
        reject_votes: reject,
        abstain_votes: abstain,
        consensus_type,
        agreement_percentage: agreement_pct,
        dissenting_opinions: dissenting,
        aggregation_method: "weighted".to_string(),
        quorum_met,
    }
}

/// Unanimous aggregation: requires ALL non-abstain votes to APPROVE.
pub fn aggregate_unanimous(reviews: &[ExpertReview], quorum: usize) -> AggregatedDecision {
    let (approve, reject, abstain) = count_votes(reviews);
    let total = reviews.len();
    let non_abstain = approve + reject;
    let quorum_met = non_abstain >= quorum;

    let non_abstain_reviews: Vec<&ExpertReview> = reviews
        .iter()
        .filter(|r| r.vote != VoteChoice::Abstain)
        .collect();
    let all_approve = !non_abstain_reviews.is_empty()
        && non_abstain_reviews
            .iter()
            .all(|r| r.vote == VoteChoice::Approve);

    let (decision, consensus_type, agreement_pct, dissenting, avg_confidence) = if all_approve {
        let avg = non_abstain_reviews
            .iter()
            .map(|r| r.confidence)
            .sum::<f32>()
            / non_abstain_reviews.len() as f32;
        (
            VoteChoice::Approve,
            "unanimous".to_string(),
            100.0,
            Vec::new(),
            avg,
        )
    } else {
        let consensus = if approve > 0 {
            "not_unanimous"
        } else {
            "unanimous_rejection"
        };
        let (pct, dissenting): (f32, Vec<ExpertReview>) =
            if reject == non_abstain && non_abstain > 0 {
                (100.0, Vec::new())
            } else if non_abstain > 0 {
                (
                    (reject as f32 / non_abstain as f32) * 100.0,
                    reviews
                        .iter()
                        .filter(|r| r.vote == VoteChoice::Approve)
                        .cloned()
                        .collect(),
                )
            } else {
                (0.0, Vec::new())
            };
        let reject_reviews: Vec<&ExpertReview> = reviews
            .iter()
            .filter(|r| r.vote == VoteChoice::Reject)
            .collect();
        let avg = if reject_reviews.is_empty() {
            0.5
        } else {
            reject_reviews.iter().map(|r| r.confidence).sum::<f32>() / reject_reviews.len() as f32
        };
        (
            VoteChoice::Reject,
            consensus.to_string(),
            pct,
            dissenting,
            avg,
        )
    };

    AggregatedDecision {
        decision,
        confidence: avg_confidence,
        total_votes: total,
        approve_votes: approve,
        reject_votes: reject,
        abstain_votes: abstain,
        consensus_type,
        agreement_percentage: agreement_pct,
        dissenting_opinions: dissenting,
        aggregation_method: "unanimous".to_string(),
        quorum_met,
    }
}

/// Build a `DissentReport` if the decision had any dissenting opinions.
pub fn generate_dissent_report(decision: &AggregatedDecision) -> Option<DissentReport> {
    if decision.dissenting_opinions.is_empty() {
        return None;
    }
    let (majority_count, dissent_count) = match decision.decision {
        VoteChoice::Approve => (decision.approve_votes, decision.reject_votes),
        _ => (decision.reject_votes, decision.approve_votes),
    };
    let dissent_experts: Vec<String> = decision
        .dissenting_opinions
        .iter()
        .map(|r| r.expert_id.clone())
        .collect();
    let dissent_rationales: Vec<String> = decision
        .dissenting_opinions
        .iter()
        .map(|r| r.vote_rationale.clone())
        .collect();
    let mut concerns: Vec<String> = decision
        .dissenting_opinions
        .iter()
        .flat_map(|r| r.weaknesses.iter().cloned())
        .collect();
    concerns.sort();
    concerns.dedup();
    Some(DissentReport {
        decision: decision.decision,
        majority_count,
        dissent_count,
        majority_experts: Vec::new(),
        dissent_experts,
        dissent_rationales,
        concerns_raised: concerns,
    })
}

#[cfg(test)]
mod inline_tests {
    use super::*;
    use std::time::Duration;

    fn rev(d: &str, v: VoteChoice, c: f32) -> ExpertReview {
        ExpertReview {
            expert_id: format!("{d}-expert"),
            domain: d.to_string(),
            analysis: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            domain_scores: Default::default(),
            vote: v,
            confidence: c,
            vote_rationale: String::new(),
            review_duration: Duration::ZERO,
        }
    }

    #[test]
    fn classify_consensus_levels() {
        assert_eq!(classify_consensus(100.0), "unanimous");
        assert_eq!(classify_consensus(80.0), "strong_majority");
        assert_eq!(classify_consensus(60.0), "simple_majority");
        assert_eq!(classify_consensus(40.0), "split");
    }

    #[test]
    fn unanimous_all_abstain_yields_reject() {
        let reviews = vec![
            rev("a", VoteChoice::Abstain, 1.0),
            rev("b", VoteChoice::Abstain, 1.0),
        ];
        let d = aggregate_unanimous(&reviews, 0);
        assert_eq!(d.decision, VoteChoice::Reject);
    }
}
