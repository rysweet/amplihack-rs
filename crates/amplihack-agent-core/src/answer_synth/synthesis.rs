//! Core LLM synthesis with intent-aware prompting.

use std::fmt::Write;
use tracing::{error, info};

use crate::agentic_loop::traits::LlmClient;
use crate::agentic_loop::types::{LlmMessage, MemoryFact};
use crate::error::AgentError;
use super::types::{DetectedIntent, IntentType, QuestionLevel, SynthesisConfig};

/// Synthesize an answer from retrieved `facts` using the LLM.
pub async fn synthesize(
    question: &str,
    facts: &[MemoryFact],
    level: QuestionLevel,
    intent: &DetectedIntent,
    llm: &dyn LlmClient,
    config: &SynthesisConfig,
) -> Result<String, AgentError> {
    if facts.is_empty() {
        return Ok("I don't have enough information to answer that question.".into());
    }
    let max_facts = if intent.intent.is_complex() { config.max_facts_complex }
                    else { config.max_facts_simple };
    let context_str = format_facts(facts, max_facts, intent);
    let extra = build_extra_instructions(question, level, intent);
    let prompt = format!(
        "Answer the following question using ONLY the provided facts.\n\n\
         Question ({level}): {question}\n\nInstructions: {i}\n{extra}\n\n{context_str}\n\n\
         Provide a clear, accurate answer.", i = level.instruction());
    info!(question = &question[..question.len().min(80)], total_facts = facts.len().min(max_facts),
        "SYNTH_DIAG starting synthesis");
    let messages = [
        LlmMessage::system("You are a knowledgeable assistant that synthesizes accurate \
            answers from retrieved facts. Answer precisely based on the provided context."),
        LlmMessage::user(prompt),
    ];
    match llm.completion(&messages, &config.model, config.synthesis_temperature).await {
        Ok(answer) => Ok(answer.trim().to_string()),
        Err(e) => {
            error!(error = %e, "LLM synthesis failed");
            Ok("I was unable to synthesize an answer due to an internal error.".into())
        }
    }
}

fn format_facts(facts: &[MemoryFact], max_facts: usize, intent: &DetectedIntent) -> String {
    let temporal = intent.needs_temporal
        || matches!(intent.intent, IntentType::TemporalComparison | IntentType::IncrementalUpdate);
    let header = if temporal { "Relevant facts (ordered chronologically where possible):\n" }
                 else { "Relevant facts:\n" };
    let mut out = String::from(header);
    for (i, fact) in facts.iter().take(max_facts).enumerate() {
        let mut markers = Vec::new();
        if temporal {
            if let Some(d) = fact.metadata.get("source_date").and_then(|v| v.as_str()) {
                markers.push(format!("Date: {d}"));
            }
            if let Some(o) = fact.metadata.get("temporal_order").and_then(|v| v.as_str()) {
                markers.push(o.to_string());
            }
        }
        if let Some(l) = fact.metadata.get("source_label").and_then(|v| v.as_str()) {
            markers.push(format!("Source: {l}"));
        }
        if fact.metadata.get("superseded").and_then(|v| v.as_bool()).unwrap_or(false) {
            markers.push("OUTDATED".into());
        }
        let m = if markers.is_empty() { String::new() } else { format!(" [{}]", markers.join(", ")) };
        let _ = writeln!(out, "{}. Context: {}{m}\n   Fact: {}\n", i + 1, fact.context, fact.outcome);
    }
    out
}

fn build_extra_instructions(question: &str, level: QuestionLevel, intent: &DetectedIntent) -> String {
    let q = question.to_lowercase();
    let mut extra = String::new();
    if intent.intent.is_complex() {
        if let Some(cat) = category_instruction(intent.intent) { extra.push_str(cat); }
        else if intent.needs_math && intent.computed_math.is_none() {
            extra.push_str("\n\nMATHEMATICAL COMPUTATION REQUIRED:\n\
                Extract raw numbers, show arithmetic step by step, double-check.\n");
        }
    }
    if let Some(ref computed) = intent.computed_math {
        let _ = write!(extra, "\n\nPRE-COMPUTED RESULT (do NOT re-calculate):\n{computed}\n");
    }
    if intent.needs_temporal
        || matches!(intent.intent, IntentType::TemporalComparison | IntentType::IncrementalUpdate)
    {
        let trap_cues = ["before the", "after the first", "before any", "originally",
            "previous", "intermediate", "i want the", "not the current", "return the"];
        if trap_cues.iter().any(|c| q.contains(c)) {
            extra.push_str("\n\nTEMPORAL TRAP: Asks for a SPECIFIC historical value. \
                Build timeline, identify position, report ONLY that value.\n");
        } else {
            extra.push_str("\n\nTEMPORAL REASONING: Reconstruct chronological chain, \
                identify the point asked about, calculate differences if asked.\n");
        }
    }
    if let Some(ref tc) = intent.temporal_code
        && let Some(ref result_val) = tc.result
    {
        let label = if tc.operation == "change_count" { "Change count" } else { "Value" };
        let _ = write!(extra, "\n\nAUTHORITATIVE RESOLUTION:\nCode: {}\n{label}: \
            {result_val:?}\nChain: {}\n", tc.code, tc.transitions.len());
    }
    append_cue_instructions(&mut extra, &q, level, intent);
    extra
}

fn category_instruction(intent: IntentType) -> Option<&'static str> {
    match intent {
        IntentType::MathematicalComputation => Some(
            "\n\nMATHEMATICAL COMPUTATION: A pre-computed result is provided. Use it directly.\n"),
        IntentType::MetaMemory => Some(
            "\n\nCOUNTING/ENUMERATION: Scan ALL facts. List each by name. Count precisely.\n"),
        IntentType::TemporalComparison => Some(
            "\n\nTEMPORAL COMPARISON: Reconstruct the FULL chronological chain before answering.\n"),
        _ => None,
    }
}

fn append_cue_instructions(extra: &mut String, q: &str, level: QuestionLevel, intent: &DetectedIntent) {
    let has = |cues: &[&str]| cues.iter().any(|c| q.contains(c));
    if has(&["incident", "cve-", "cve ", "apt-", "apt ", "breach", "attack",
             "vulnerability", "timeline", "forensic"]) {
        extra.push_str("\n\nINCIDENT TRACKING: Include ALL CVEs, timeline events, APT attributions.\n");
    }
    if has(&["how are", "relationship between", "connect", "in common",
             "relate to", "link between", "both", "and also"]) {
        extra.push_str("\n\nMULTI-HOP: Address EACH entity, then explain the CONNECTION.\n");
    }
    if intent.intent == IntentType::MultiSourceSynthesis {
        extra.push_str("\n\nMULTI-SOURCE: Identify sources, combine information across them.\n");
    }
    if intent.intent == IntentType::ContradictionResolution || level == QuestionLevel::L5
        || has(&["disagree", "conflicting", "contradiction", "reliable", "trust"]) {
        extra.push_str("\n\nCONFLICTING INFO: Present ALL conflicting values with sources.\n");
    }
    if intent.intent == IntentType::CausalCounterfactual
        || has(&["what if", "if ", "would ", "without ", "had not", "removed"]) {
        extra.push_str("\n\nCOUNTERFACTUAL: Start from facts, apply hypothetical, reason through.\n");
    }
    if intent.intent == IntentType::CausalCounterfactual
        || has(&["cause", "caused", "why did", "most important", "root cause", "single factor"]) {
        extra.push_str("\n\nCAUSAL: Distinguish root causes, contributing factors, proximate causes.\n");
    }
    if intent.intent == IntentType::RatioTrendAnalysis
        || (intent.intent.is_complex() && has(&["ratio", "rate", "per ", "trend", "improving"])) {
        extra.push_str("\n\nRATIO/TREND: Compute ratios per entity/period, analyze trend direction.\n");
    }
    if has(&["be careful", "be precise", "do not confuse", "don't confuse",
             "note:", "note that", "i want the", "not the current", "not any other"]) {
        extra.push_str("\n\nPRECISION: State ONLY the correct answer. SHORT and DIRECT.\n");
    }
    if has(&["total across", "sum of", "combined", "all projects", "across all",
             "total budget", "total cost", "per user"]) {
        extra.push_str("\n\nAGGREGATION: List EACH entity's contribution. Show the addition.\n");
    }
    if intent.intent == IntentType::MathematicalComputation
        || (intent.needs_math && intent.computed_math.is_none()) {
        extra.push_str("\n\nSTEP-BY-STEP: Extract ALL relevant numbers. Show each arithmetic step.\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fact(ctx: &str, out: &str) -> MemoryFact {
        MemoryFact { id: ctx.into(), context: ctx.into(), outcome: out.into(),
            confidence: 1.0, metadata: HashMap::new() }
    }

    #[test]
    fn format_facts_basic() {
        let s = format_facts(&[fact("dogs", "are mammals")], 10, &DetectedIntent::default());
        assert!(s.contains("Relevant facts:") && s.contains("dogs") && s.contains("are mammals"));
        assert!(!s.contains("chronologically"));
    }

    #[test]
    fn format_facts_temporal() {
        let mut meta = HashMap::new();
        meta.insert("source_date".into(), serde_json::Value::String("2024-01-01".into()));
        let f = MemoryFact { id: "t1".into(), context: "event".into(), outcome: "happened".into(),
            confidence: 1.0, metadata: meta };
        let i = DetectedIntent { needs_temporal: true, ..Default::default() };
        let s = format_facts(&[f], 10, &i);
        assert!(s.contains("chronologically") && s.contains("Date: 2024-01-01"));
    }

    #[test]
    fn extra_for_incident() {
        let e = build_extra_instructions("CVE-2024-001 breach?", QuestionLevel::L1, &DetectedIntent::default());
        assert!(e.contains("INCIDENT"));
    }

    #[test]
    fn extra_for_contradiction() {
        let i = DetectedIntent { intent: IntentType::ContradictionResolution, ..Default::default() };
        let e = build_extra_instructions("reliable?", QuestionLevel::L5, &i);
        assert!(e.contains("CONFLICTING"));
    }

    #[test]
    fn extra_temporal_trap() {
        let i = DetectedIntent { needs_temporal: true, ..Default::default() };
        let e = build_extra_instructions("value before the first change?", QuestionLevel::L2, &i);
        assert!(e.contains("TEMPORAL TRAP"));
    }

    #[test]
    fn extra_temporal_normal() {
        let i = DetectedIntent { intent: IntentType::TemporalComparison, needs_temporal: true,
            ..Default::default() };
        let e = build_extra_instructions("How did value change?", QuestionLevel::L2, &i);
        assert!(e.contains("TEMPORAL REASONING"));
    }

    #[test]
    fn extra_math_precomputed() {
        let i = DetectedIntent { intent: IntentType::MathematicalComputation, needs_math: true,
            computed_math: Some("42".into()), ..Default::default() };
        let e = build_extra_instructions("6*7?", QuestionLevel::L1, &i);
        assert!(e.contains("PRE-COMPUTED") && e.contains("42"));
    }

    #[test]
    fn category_dispatch() {
        assert!(category_instruction(IntentType::MathematicalComputation).is_some());
        assert!(category_instruction(IntentType::MetaMemory).is_some());
        assert!(category_instruction(IntentType::SimpleRecall).is_none());
    }
}
