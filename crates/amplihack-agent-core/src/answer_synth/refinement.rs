//! Agentic refinement loop: evaluate completeness → gap-fill → re-synthesize.

use std::collections::HashSet;

use amplihack_utils::parse_llm_json;
use tracing::{info, warn};

use crate::agentic_loop::traits::{LlmClient, MemoryRetriever};
use crate::agentic_loop::types::{LlmMessage, MemoryFact, ReasoningTrace};
use crate::error::AgentError;

use super::intent::detect_intent;
use super::retrieval::{adaptive_retrieve, dedup_and_filter};
use super::synthesis::synthesize;
use super::types::{CompletenessEvaluation, DetectedIntent, QuestionLevel, SynthesisConfig};

/// Single-shot answer: intent → retrieve → synthesize. Returns `(answer, trace)`.
pub async fn answer_question(
    question: &str,
    level: QuestionLevel,
    llm: &dyn LlmClient,
    retriever: &dyn MemoryRetriever,
    config: &SynthesisConfig,
) -> Result<(String, ReasoningTrace), AgentError> {
    if question.trim().is_empty() {
        return Ok(("Error: Question is empty".into(), ReasoningTrace::default()));
    }
    let intent = detect_intent(question, llm, &config.model).await?;
    let outcome = adaptive_retrieve(question, &intent, retriever, config);
    if outcome.facts.is_empty() {
        return Ok((
            "I don't have enough information to answer that question.".into(),
            build_trace(question, &intent, 0, true),
        ));
    }
    let facts = dedup_and_filter(outcome.facts);
    let answer = synthesize(question, &facts, level, &intent, llm, config).await?;
    Ok((
        answer,
        build_trace(question, &intent, facts.len(), outcome.used_simple_path),
    ))
}

/// Agentic answer: single-shot first, then evaluate + gap-fill + re-synthesize.
pub async fn answer_question_agentic(
    question: &str,
    max_iterations: usize,
    llm: &dyn LlmClient,
    retriever: &dyn MemoryRetriever,
    config: &SynthesisConfig,
) -> Result<(String, ReasoningTrace), AgentError> {
    if question.trim().is_empty() {
        return Ok(("Error: Question is empty".into(), ReasoningTrace::default()));
    }
    let (initial_answer, trace) =
        answer_question(question, QuestionLevel::L3, llm, retriever, config).await?;
    let evaluation = evaluate_completeness(question, &initial_answer, llm, config).await?;
    if evaluation.is_complete || evaluation.gaps.is_empty() {
        info!("Agentic: single-shot answer is complete");
        return Ok((initial_answer, trace));
    }
    let mut additional = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for gap in evaluation.gaps.iter().take(max_iterations) {
        for f in retriever.search(gap, 50) {
            if !f.id.is_empty() && seen.insert(f.id.clone()) {
                additional.push(f);
            }
        }
    }
    if additional.is_empty() {
        info!("Agentic: no additional facts for gaps");
        return Ok((initial_answer, trace));
    }
    let original = retriever.search(question, config.max_retrieval_limit);
    let prev_fact = MemoryFact {
        id: "__previous_answer__".into(),
        context: "PREVIOUS_ANSWER".into(),
        outcome: format!("A previous analysis answered: {initial_answer}"),
        confidence: 0.95,
        metadata: Default::default(),
    };
    let mut all = vec![prev_fact];
    all.extend(original);
    all.extend(additional);
    let deduped = dedup_and_filter(all);
    let intent = detect_intent(question, llm, &config.model).await?;
    let refined = synthesize(question, &deduped, QuestionLevel::L3, &intent, llm, config).await?;
    info!(
        total = deduped.len(),
        "Agentic: refined with additional facts"
    );
    Ok((refined, trace))
}

/// Evaluate whether `answer` fully addresses `question`.
pub async fn evaluate_completeness(
    question: &str,
    answer: &str,
    llm: &dyn LlmClient,
    config: &SynthesisConfig,
) -> Result<CompletenessEvaluation, AgentError> {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Ok(incomplete_needs_refinement(question));
    }
    let lower = trimmed.to_lowercase();
    let no_info = [
        "i don't have enough",
        "i don't have information",
        "i cannot answer",
        "no information available",
        "not enough context",
    ];
    if no_info.iter().any(|p| lower.starts_with(p)) {
        return Ok(incomplete_needs_refinement(question));
    }
    let prompt = format!(
        "Evaluate if this answer FULLY addresses the question.\n\n\
         QUESTION: {question}\nANSWER: {answer}\n\n\
         Respond with JSON: {{\"is_complete\": true}} or \
         {{\"is_complete\": false, \"gaps\": [\"search query\"]}}\n\
         Only mark incomplete if SPECIFIC information is missing. Return ONLY JSON."
    );
    let raw = llm
        .completion(
            &[LlmMessage::user(prompt)],
            &config.model,
            config.eval_temperature,
        )
        .await?;
    // issue #868: the LLM evaluation ALWAYS runs (there is no `len > 50`
    // short-circuit that would declare a long answer complete without eval), and
    // a parse failure fails toward "not complete" while recording the question as
    // a gap so the agentic refinement loop actually continues instead of silently
    // accepting a possibly-incomplete answer.
    Ok(parse_completeness_value(&raw).unwrap_or_else(|| {
        warn!("completeness evaluation returned unparseable output; failing closed (not complete)");
        incomplete_needs_refinement(question)
    }))
}

/// A fail-closed completeness result: not complete, with `question` re-queued as
/// the gap to refine.
///
/// issue #868: an empty answer, an explicit "no information" answer, and a parse
/// failure all fail toward "not complete" and record the question as a gap so
/// the agentic refinement loop keeps searching instead of silently accepting a
/// possibly-incomplete answer.
fn incomplete_needs_refinement(question: &str) -> CompletenessEvaluation {
    CompletenessEvaluation {
        is_complete: false,
        gaps: vec![question.into()],
    }
}

/// Parse a completeness-evaluation JSON payload.
///
/// Returns `None` when no JSON payload could be extracted (missing or corrupt),
/// which callers treat as a fail-closed signal. When a payload is present but
/// omits `is_complete`, it defaults to `false` (issue #868: an absent field must
/// never be read as completion).
fn parse_completeness_value(raw: &str) -> Option<CompletenessEvaluation> {
    let value = parse_llm_json(raw)?;
    Some(CompletenessEvaluation {
        is_complete: value
            .get("is_complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        gaps: value
            .get("gaps")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Test-only helper that encodes the fail-closed contract for a completeness
/// payload. Production code uses [`parse_completeness_value`] directly so it can
/// distinguish a parse failure (to inject the question as a gap).
#[cfg(test)]
fn parse_completeness_response(raw: &str) -> CompletenessEvaluation {
    // issue #868: a parse failure fails CLOSED (not complete), never silently
    // declares the answer complete. `Default` for `CompletenessEvaluation` is
    // exactly `{ is_complete: false, gaps: [] }`.
    parse_completeness_value(raw).unwrap_or_default()
}

fn build_trace(
    question: &str,
    intent: &DetectedIntent,
    facts: usize,
    simple: bool,
) -> ReasoningTrace {
    let mut m = std::collections::HashMap::new();
    m.insert(
        "intent".into(),
        serde_json::Value::String(intent.intent.to_string()),
    );
    m.insert(
        "needs_temporal".into(),
        serde_json::Value::Bool(intent.needs_temporal),
    );
    m.insert(
        "needs_math".into(),
        serde_json::Value::Bool(intent.needs_math),
    );
    ReasoningTrace {
        question: question.into(),
        intent: m,
        total_facts_collected: facts,
        used_simple_path: simple,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;

    struct MockLlm(String);
    #[async_trait]
    impl LlmClient for MockLlm {
        async fn completion(
            &self,
            _: &[LlmMessage],
            _: &str,
            _: f64,
        ) -> Result<String, AgentError> {
            Ok(self.0.clone())
        }
    }

    struct MockRetriever(Vec<MemoryFact>);
    impl MemoryRetriever for MockRetriever {
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

    #[test]
    fn parse_completeness_variants() {
        let c = parse_completeness_response(r#"{"is_complete": true}"#);
        assert!(c.is_complete && c.gaps.is_empty());
        let c = parse_completeness_response(r#"{"is_complete": false, "gaps": ["a","b"]}"#);
        assert!(!c.is_complete && c.gaps.len() == 2);
        // issue #868: a parse failure must fail CLOSED (not complete), never
        // silently declare the answer complete.
        assert!(!parse_completeness_response("garbage").is_complete);
        // Fenced JSON still parses.
        let c = parse_completeness_response(
            "```json\n{\"is_complete\": false, \"gaps\": [\"x\"]}\n```",
        );
        assert!(!c.is_complete && c.gaps == vec!["x"]);
        // Robust extraction: JSON embedded in prose (the inline fence strip could not).
        let c = parse_completeness_response(
            "Evaluation: {\"is_complete\": false, \"gaps\": [\"y\"]} done",
        );
        assert!(!c.is_complete && c.gaps == vec!["y"]);
    }

    #[test]
    fn parse_completeness_fieldless_is_not_complete() {
        // issue #868: a present-but-fieldless payload must NOT read as success.
        // The old code used `.unwrap_or(true)`; the default now flips to `false`
        // so an absent `is_complete` field can no longer be read as completion.
        assert!(!parse_completeness_response("{}").is_complete);
        let c = parse_completeness_response(r#"{"gaps": ["missing detail"]}"#);
        assert!(!c.is_complete);
        assert_eq!(c.gaps, vec!["missing detail"]);
    }

    #[tokio::test]
    async fn answer_empty() {
        let (a, _) = answer_question(
            "",
            QuestionLevel::L1,
            &MockLlm("{}".into()),
            &MockRetriever(vec![]),
            &SynthesisConfig::default(),
        )
        .await
        .unwrap();
        assert!(a.contains("empty"));
    }

    #[tokio::test]
    async fn answer_no_facts() {
        let llm = MockLlm(
            r#"{"intent":"simple_recall","needs_temporal":false,"needs_math":false}"#.into(),
        );
        let (a, t) = answer_question(
            "What is X?",
            QuestionLevel::L1,
            &llm,
            &MockRetriever(vec![]),
            &SynthesisConfig::default(),
        )
        .await
        .unwrap();
        assert!(a.contains("don't have enough"));
        assert!(t.used_simple_path);
    }

    #[tokio::test]
    async fn answer_with_facts() {
        let (a, t) = answer_question(
            "Do dogs have fur?",
            QuestionLevel::L2,
            &MockLlm("Dogs are mammals with fur.".into()),
            &MockRetriever(vec![mf("f1", "Dogs", "are mammals")]),
            &SynthesisConfig::default(),
        )
        .await
        .unwrap();
        assert!(a.contains("Dogs") || a.contains("mammals"));
        assert!(t.total_facts_collected > 0);
    }

    #[tokio::test]
    async fn completeness_cases() {
        let cfg = SynthesisConfig::default();
        // Empty answer -> not complete (short-circuits before any LLM call).
        let llm = MockLlm(r#"{"is_complete": true}"#.into());
        assert!(
            !(evaluate_completeness("Q?", "", &llm, &cfg)
                .await
                .unwrap()
                .is_complete)
        );
        // "no info" answer -> not complete.
        assert!(
            !(evaluate_completeness("Q?", "I don't have enough", &llm, &cfg)
                .await
                .unwrap()
                .is_complete)
        );
        // issue #868: length alone must NOT short-circuit to complete — the LLM
        // evaluation always runs. A long answer the model marks incomplete stays
        // incomplete (the old `len > 50` path declared it complete without eval).
        let long = "A very detailed and comprehensive answer that addresses the question fully.";
        let incomplete_llm = MockLlm(r#"{"is_complete": false, "gaps": ["pricing"]}"#.into());
        let e = evaluate_completeness("Q?", long, &incomplete_llm, &cfg)
            .await
            .unwrap();
        assert!(!e.is_complete);
        assert_eq!(e.gaps, vec!["pricing"]);
        // A long answer the model confirms complete -> complete.
        let complete_llm = MockLlm(r#"{"is_complete": true}"#.into());
        assert!(
            evaluate_completeness("Q?", long, &complete_llm, &cfg)
                .await
                .unwrap()
                .is_complete
        );
    }

    #[tokio::test]
    async fn evaluate_completeness_parse_failure_fails_closed() {
        // issue #868: on a parse miss the evaluation must fail toward "not
        // complete" AND record a gap, so the agentic refinement loop actually
        // continues. An empty gap list is treated as "done" by
        // `answer_question_agentic`, so failing closed also requires a gap.
        let cfg = SynthesisConfig::default();
        let long = "A very detailed and comprehensive answer that addresses the question fully.";
        let garbage_llm = MockLlm("totally not json".into());
        let e = evaluate_completeness("What is the price?", long, &garbage_llm, &cfg)
            .await
            .unwrap();
        assert!(
            !e.is_complete,
            "parse failure must not declare completeness"
        );
        assert!(
            e.gaps.iter().any(|g| g == "What is the price?"),
            "parse failure must record the question as a gap so refinement continues"
        );
    }

    #[tokio::test]
    async fn agentic_empty() {
        let (a, _) = answer_question_agentic(
            "  ",
            3,
            &MockLlm("{}".into()),
            &MockRetriever(vec![]),
            &SynthesisConfig::default(),
        )
        .await
        .unwrap();
        assert!(a.contains("empty"));
    }

    #[test]
    fn trace_fields() {
        let i = DetectedIntent {
            intent: crate::answer_synth::IntentType::TemporalComparison,
            needs_temporal: true,
            ..Default::default()
        };
        let t = build_trace("q?", &i, 10, false);
        assert_eq!(t.question, "q?");
        assert_eq!(t.total_facts_collected, 10);
        assert!(!t.used_simple_path);
    }
}
