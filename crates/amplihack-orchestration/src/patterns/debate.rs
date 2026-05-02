//! Multi-agent debate orchestrator.
//!
//! Native Rust port of `patterns/debate.py`. Runs `rounds * len(perspectives)`
//! parallel calls + 1 facilitator synthesis. Returns the synthesis with a
//! parsed confidence level.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::claude_process::{ProcessResult, ProcessRunner};
use crate::execution::run_parallel;
use crate::session::OrchestratorSession;

/// Profile for a single debate perspective.
#[derive(Debug, Clone, Copy)]
pub struct Perspective {
    pub name: &'static str,
    pub focus: &'static str,
    pub questions: &'static str,
}

/// Default 5 standard perspectives.
pub static DEFAULT_PERSPECTIVES: &[Perspective] = &[
    Perspective {
        name: "security",
        focus: "Vulnerabilities, attack vectors, data protection",
        questions: "What could go wrong? How do we prevent breaches?",
    },
    Perspective {
        name: "performance",
        focus: "Speed, scalability, resource efficiency",
        questions: "Will this scale? What are the bottlenecks?",
    },
    Perspective {
        name: "simplicity",
        focus: "Minimal complexity, ruthless simplification",
        questions: "Is this the simplest solution? Can we remove abstractions?",
    },
    Perspective {
        name: "maintainability",
        focus: "Long-term evolution, technical debt",
        questions: "Can future developers understand this? How hard to change?",
    },
    Perspective {
        name: "user_experience",
        focus: "API design, usability, developer experience",
        questions: "Is this intuitive? How will users interact with this?",
    },
];

/// Confidence rating returned by the facilitator synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
    None,
}

/// Per-round results.
#[derive(Debug, Clone)]
pub struct RoundData {
    pub round: usize,
    pub round_type: &'static str,
    pub results: HashMap<String, ProcessResult>,
}

/// Outcome of a debate run.
#[derive(Debug, Clone)]
pub struct DebateResult {
    pub rounds: Vec<RoundData>,
    pub positions: HashMap<String, Vec<String>>,
    pub synthesis: Option<ProcessResult>,
    pub confidence: Confidence,
    pub session_id: String,
    pub success: bool,
}

/// Execute the multi-agent debate pattern.
#[allow(clippy::too_many_arguments)]
pub async fn run_debate(
    decision_question: String,
    perspectives: Option<Vec<String>>,
    rounds: usize,
    model: Option<String>,
    working_dir: Option<PathBuf>,
    timeout: Option<Duration>,
    runner: Arc<dyn ProcessRunner>,
) -> DebateResult {
    let working_dir = working_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let perspective_names: Vec<String> = perspectives.unwrap_or_else(|| {
        vec![
            "security".to_string(),
            "performance".to_string(),
            "simplicity".to_string(),
        ]
    });

    let mut session = OrchestratorSession::builder()
        .pattern_name("debate")
        .working_dir(working_dir);
    if let Some(m) = model.clone() {
        session = session.model(m);
    }
    let mut session = session.runner(runner).build().expect("session build");

    session.log_info("Starting Multi-Agent Debate");
    session.log_info(&format!("Decision: {}", decision_question));
    session.log_info(&format!("Perspectives: {}", perspective_names.join(", ")));
    session.log_info(&format!("Rounds: {rounds}"));

    let profile_lookup: HashMap<&str, Perspective> =
        DEFAULT_PERSPECTIVES.iter().map(|p| (p.name, *p)).collect();

    let mut history: HashMap<String, Vec<String>> = perspective_names
        .iter()
        .map(|n| (n.clone(), Vec::new()))
        .collect();
    let mut all_rounds: Vec<RoundData> = Vec::new();

    // Round 1: initial positions.
    let mut processes = Vec::with_capacity(perspective_names.len());
    for name in &perspective_names {
        let prompt =
            build_round1_prompt(&decision_question, name, profile_lookup.get(name.as_str()));
        let pid = format!("round1_{name}");
        let p = session
            .create_process(&prompt, Some(&pid), None, timeout)
            .expect("create_process");
        processes.push((name.clone(), p));
    }
    let names_only: Vec<String> = processes.iter().map(|(n, _)| n.clone()).collect();
    let r1 = run_parallel(processes.into_iter().map(|(_, p)| p).collect(), None).await;

    let mut r1_data = HashMap::new();
    for (name, result) in names_only.iter().zip(r1.iter()) {
        history.get_mut(name).unwrap().push(result.output.clone());
        r1_data.insert(name.clone(), result.clone());
    }
    all_rounds.push(RoundData {
        round: 1,
        round_type: "initial_positions",
        results: r1_data,
    });

    let r1_success = r1.iter().filter(|r| r.is_success()).count();
    if r1_success == 0 {
        session.log_error("All perspectives failed in Round 1");
        return DebateResult {
            rounds: all_rounds,
            positions: history,
            synthesis: None,
            confidence: Confidence::None,
            session_id: session.session_id().to_string(),
            success: false,
        };
    }

    // Rounds 2..N
    for round_num in 2..=rounds {
        let prev_context = build_previous_context(&perspective_names, &history);
        let mut processes = Vec::with_capacity(perspective_names.len());
        for name in &perspective_names {
            let prompt = build_roundn_prompt(
                &decision_question,
                round_num,
                name,
                profile_lookup.get(name.as_str()),
                &prev_context,
            );
            let pid = format!("round{round_num}_{name}");
            let p = session
                .create_process(&prompt, Some(&pid), None, timeout)
                .expect("create_process");
            processes.push((name.clone(), p));
        }
        let names_only: Vec<String> = processes.iter().map(|(n, _)| n.clone()).collect();
        let rn = run_parallel(processes.into_iter().map(|(_, p)| p).collect(), None).await;
        let mut rn_data = HashMap::new();
        for (name, result) in names_only.iter().zip(rn.iter()) {
            history.get_mut(name).unwrap().push(result.output.clone());
            rn_data.insert(name.clone(), result.clone());
        }
        all_rounds.push(RoundData {
            round: round_num,
            round_type: "challenge_respond",
            results: rn_data,
        });
    }

    // Synthesis.
    let transcript = build_transcript(&all_rounds, &perspective_names);
    let synth_prompt = build_synthesis_prompt(&decision_question, &perspective_names, &transcript);
    let synth_proc = session
        .create_process(&synth_prompt, Some("facilitator_synthesis"), None, timeout)
        .expect("create_process");
    let synth = synth_proc.run().await;

    let confidence = parse_confidence(&synth, &all_rounds, perspective_names.len(), rounds);
    let success = synth.is_success();
    session.log_info(&format!("Debate complete. Confidence: {:?}", confidence));

    DebateResult {
        rounds: all_rounds,
        positions: history,
        synthesis: Some(synth),
        confidence,
        session_id: session.session_id().to_string(),
        success,
    }
}

fn build_round1_prompt(question: &str, name: &str, profile: Option<&Perspective>) -> String {
    let (focus, questions) = profile
        .map(|p| (p.focus.to_string(), p.questions.to_string()))
        .unwrap_or_else(|| {
            (
                format!("{name} considerations"),
                format!("What {name} aspects should we consider?"),
            )
        });
    format!(
        "You are participating in a structured multi-agent debate.\n\n\
         DECISION QUESTION:\n{question}\n\n\
         YOUR ROLE: {name} Perspective\nFOCUS: {focus}\nKEY QUESTIONS: {questions}\n\n\
         This is ROUND 1: Form your initial position on this decision.\n\n\
         ## Recommendation\n[Your recommended approach]\n\n\
         ## Supporting Arguments\n1. [Argument]\n2. [Argument]\n\n\
         ## Risks of Alternatives\n- [Concerns]\n\n\
         ## Assumptions\n- [Assumption]\n",
    )
}

fn build_roundn_prompt(
    question: &str,
    round: usize,
    name: &str,
    profile: Option<&Perspective>,
    previous_context: &str,
) -> String {
    let focus = profile
        .map(|p| p.focus.to_string())
        .unwrap_or_else(|| format!("{name} considerations"));
    format!(
        "You are participating in a structured multi-agent debate.\n\n\
         DECISION QUESTION:\n{question}\n\n\
         YOUR ROLE: {name} Perspective\nFOCUS: {focus}\n\n\
         This is ROUND {round}: Challenge other perspectives and defend your position.\n\n\
         PREVIOUS ROUND POSITIONS:\n{previous_context}\n\n\
         ## Challenges to Other Perspectives\n[...]\n\n\
         ## Defense of My Position\n[...]\n\n\
         ## Concessions\n[...]\n\n\
         ## Refined Position\n[...]\n\n\
         ## Common Ground Identified\n[...]\n",
    )
}

fn build_previous_context(
    perspective_names: &[String],
    history: &HashMap<String, Vec<String>>,
) -> String {
    let mut parts = Vec::new();
    for name in perspective_names {
        if let Some(hist) = history.get(name)
            && let Some(last) = hist.last()
        {
            parts.push(format!(
                "## {} Perspective (Previous Round):\n{}",
                name, last
            ));
        }
    }
    parts.join("\n\n")
}

fn build_transcript(all_rounds: &[RoundData], perspective_names: &[String]) -> String {
    let bar = "=".repeat(80);
    let mut transcript = String::new();
    for round in all_rounds {
        transcript.push_str(&format!("\n{bar}\nROUND {}\n{bar}\n", round.round));
        for name in perspective_names {
            if let Some(r) = round.results.get(name)
                && r.is_success()
            {
                transcript.push_str(&format!("\n## {} PERSPECTIVE:\n", name.to_uppercase()));
                let truncated = crate::text_utils::truncate_at_char_boundary(&r.output, 3000);
                transcript.push_str(truncated);
                if truncated.len() < r.output.len() {
                    transcript.push_str("\n...(truncated)");
                }
            }
        }
    }
    transcript
}

fn build_synthesis_prompt(question: &str, perspectives: &[String], transcript: &str) -> String {
    format!(
        "You are a neutral facilitator synthesizing a multi-perspective debate.\n\n\
         DECISION QUESTION:\n{question}\n\n\
         PERSPECTIVES INVOLVED:\n{}\n\n\
         COMPLETE DEBATE TRANSCRIPT:\n{transcript}\n\n\
         ## Recommendation\n[...]\n\n\
         ## Confidence Level\n[HIGH/MEDIUM/LOW]\n\n\
         ## Rationale\n[...]\n\n\
         ## Key Arguments That Won\n1. [...]\n\n\
         ## Dissenting Views\n[...]\n\n\
         ## Implementation Guidance\n[...]\n\n\
         ## Success Metrics\n[...]\n\n\
         ## Revisit Triggers\n[...]\n",
        perspectives.join(", "),
    )
}

fn parse_confidence(
    synth: &ProcessResult,
    all_rounds: &[RoundData],
    n_perspectives: usize,
    rounds: usize,
) -> Confidence {
    if !synth.is_success() {
        return Confidence::Low;
    }
    let lower = synth.output.to_lowercase();
    let mut confidence = Confidence::Medium;
    if lower.contains("confidence level") {
        if lower.contains("high") {
            confidence = Confidence::High;
        } else if lower.contains("low") {
            confidence = Confidence::Low;
        }
    }

    let total: usize = all_rounds
        .iter()
        .map(|r| r.results.values().filter(|v| v.is_success()).count())
        .sum();
    let expected = n_perspectives * rounds;
    if expected > 0 {
        if total == expected {
            confidence = Confidence::High;
        } else if (total as f32) < (expected as f32) * 0.5 {
            confidence = Confidence::Low;
        }
    }
    confidence
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn perspectives_count() {
        assert_eq!(DEFAULT_PERSPECTIVES.len(), 5);
    }

    #[test]
    fn confidence_medium_is_default() {
        let synth = ProcessResult::ok("just text".into(), "s".into(), Duration::ZERO);
        // 0 perspectives means expected=0 so falls through to medium.
        assert_eq!(parse_confidence(&synth, &[], 0, 0), Confidence::Medium);
    }

    #[test]
    fn confidence_low_when_synth_failed() {
        let synth = ProcessResult::err("dead".into(), "s".into(), Duration::ZERO);
        assert_eq!(parse_confidence(&synth, &[], 1, 1), Confidence::Low);
    }
}
