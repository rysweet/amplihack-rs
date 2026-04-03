//! LLM-driven code generation for temporal queries.
//!
//! Port of Python `code_synthesis.py` — generates deterministic code snippets
//! that resolve temporal questions by indexing into transition chains.

use serde::{Deserialize, Serialize};

use crate::temporal_reasoning::{
    collapse_change_count_transitions, collapse_temporal_lookup_transitions,
    heuristic_temporal_entity_field, parse_temporal_index, transition_chain_from_facts, Transition,
};

use amplihack_memory::Fact;

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

/// Result of temporal code synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSynthesisResult {
    /// Generated pseudo-code.
    pub code: String,
    /// The index expression used (e.g. `"-1"`, `"0"`).
    pub index_expr: String,
    /// The effective transition chain after collapsing.
    pub transitions: Vec<Transition>,
    /// The resolved value (if the chain is non-empty).
    pub result: Option<String>,
    /// `"change_count"` or `"state_lookup"`.
    pub operation: String,
    /// Number of effective transitions.
    pub state_count: usize,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Synthesize code to resolve a temporal question deterministically.
///
/// Produces a code snippet that retrieves the transition chain for the
/// entity/field and either indexes into it or counts the transitions.
pub fn temporal_code_synthesis(
    question: &str,
    entity: &str,
    field: &str,
    candidate_facts: &[Fact],
) -> CodeSynthesisResult {
    let lower = question.to_lowercase();
    let change_count_question = (lower.contains("how many times")
        || lower.contains("how many changes")
        || lower.contains("number of changes"))
        && ["change", "changed", "update", "updated", "modification"]
            .iter()
            .any(|t| lower.contains(t));

    let index_expr = if change_count_question {
        "max(0, len(transitions) - 1)".to_string()
    } else {
        parse_temporal_index(question)
    };

    let mut code_lines = vec![
        format!("transitions = retrieve_transition_chain({entity:?}, {field:?})"),
        format!("# Temporal index: {index_expr}"),
    ];

    if change_count_question {
        code_lines.push(format!(
            "states = _collapse_change_count_transitions(transitions, {field:?})"
        ));
        code_lines.push("# Count transitions after collapsing recap/duplicate states".into());
        code_lines.push("answer = max(0, len(states) - 1)".into());
    } else if index_expr.starts_with("len(") || index_expr == "mid" {
        code_lines.push("transitions = _collapse_temporal_lookup_transitions(transitions)".into());
        code_lines.push(format!("idx = {index_expr}"));
        code_lines.push("answer = transitions[idx].value".into());
    } else {
        code_lines.push("transitions = _collapse_temporal_lookup_transitions(transitions)".into());
        code_lines.push(format!("answer = transitions[{index_expr}].value"));
    }

    let code = code_lines.join("\n");
    let transitions = transition_chain_from_facts(entity, field, candidate_facts);

    let effective = if change_count_question {
        collapse_change_count_transitions(&transitions, field)
    } else {
        collapse_temporal_lookup_transitions(&transitions)
    };

    let result = resolve_temporal_result(&effective, &index_expr, change_count_question);

    CodeSynthesisResult {
        code,
        index_expr,
        transitions: effective.clone(),
        result,
        operation: if change_count_question {
            "change_count".into()
        } else {
            "state_lookup".into()
        },
        state_count: effective.len(),
    }
}

/// Try to synthesize a temporal result using heuristic entity/field extraction.
///
/// Returns `None` if the question doesn't match the heuristic pattern.
pub fn try_heuristic_synthesis(
    question: &str,
    candidate_facts: &[Fact],
) -> Option<CodeSynthesisResult> {
    let (entity, field) = heuristic_temporal_entity_field(question)?;
    Some(temporal_code_synthesis(question, &entity, &field, candidate_facts))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_temporal_result(
    effective: &[Transition],
    index_expr: &str,
    change_count: bool,
) -> Option<String> {
    if effective.is_empty() {
        return None;
    }

    if change_count {
        let count = effective.len().saturating_sub(1);
        return Some(count.to_string());
    }

    if index_expr == "-1" {
        let latest = effective
            .iter()
            .rev()
            .find(|t| !t.superseded)
            .or(effective.last());
        return latest.map(|t| t.value.clone());
    }

    if index_expr.starts_with("len(") || index_expr == "mid" {
        let idx = effective.len() / 2;
        return effective.get(idx).map(|t| t.value.clone());
    }

    if let Ok(idx) = index_expr.parse::<i64>() {
        let len = effective.len() as i64;
        let actual = if idx < 0 { len + idx } else { idx };
        if actual >= 0 && actual < len {
            return Some(effective[actual as usize].value.clone());
        }
    }

    None
}

/// Format a temporal lookup answer sentence.
pub fn format_temporal_lookup_answer(
    question: &str,
    result: &CodeSynthesisResult,
) -> String {
    let res = result
        .result
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if res.is_empty() {
        return res;
    }

    let cap = match crate::temporal_reasoning::heuristic_temporal_entity_field(question) {
        Some((entity, field)) => {
            // reconstruct qualifier from the question
            let lower = question.to_lowercase();
            let qualifier = ["current", "latest", "original", "initial", "previous", "final", "last"]
                .iter()
                .find(|q| lower.contains(**q))
                .unwrap_or(&"current");
            (qualifier.to_string(), field, entity)
        }
        None => return res,
    };

    let (qualifier, field, entity) = cap;
    let (descriptor, verb) = match qualifier.as_str() {
        "current" | "latest" | "final" | "last" => ("current", "is"),
        "original" | "initial" => ("original", "was"),
        other => (other, "was"),
    };

    format!("The {descriptor} {field} for {entity} {verb} {res}.")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_facts() -> Vec<Fact> {
        vec![
            Fact::new("Atlas deadline", "deadline changed from March 15 to April 1"),
            Fact::new("Atlas deadline", "current deadline is April 1"),
        ]
    }

    #[test]
    fn synthesis_state_lookup() {
        let facts = make_facts();
        let r = temporal_code_synthesis("What is the current deadline for Atlas?", "Atlas", "deadline", &facts);
        assert_eq!(r.operation, "state_lookup");
        assert!(r.code.contains("retrieve_transition_chain"));
    }

    #[test]
    fn synthesis_change_count() {
        let facts = make_facts();
        let r = temporal_code_synthesis(
            "How many times has the deadline changed for Atlas?",
            "Atlas", "deadline", &facts,
        );
        assert_eq!(r.operation, "change_count");
        assert!(r.code.contains("_collapse_change_count_transitions"));
    }

    #[test]
    fn heuristic_synthesis_works() {
        let facts = make_facts();
        let r = try_heuristic_synthesis("What is the current deadline for Atlas?", &facts);
        assert!(r.is_some());
    }

    #[test]
    fn heuristic_synthesis_none() {
        let r = try_heuristic_synthesis("Hello world", &[]);
        assert!(r.is_none());
    }

    #[test]
    fn format_answer_basic() {
        let result = CodeSynthesisResult {
            code: String::new(),
            index_expr: "-1".into(),
            transitions: Vec::new(),
            result: Some("April 1".into()),
            operation: "state_lookup".into(),
            state_count: 2,
        };
        let answer = format_temporal_lookup_answer(
            "What is the current deadline for Atlas project?",
            &result,
        );
        assert!(answer.contains("April 1"), "got: {answer}");
        assert!(answer.contains("current"));
    }

    #[test]
    fn resolve_empty_transitions() {
        let r = resolve_temporal_result(&[], "-1", false);
        assert!(r.is_none());
    }

    #[test]
    fn resolve_negative_index() {
        let t = Transition {
            value: "v1".into(),
            timestamp: String::new(),
            temporal_index: 0,
            experience_id: String::new(),
            sequence_position: 0,
            superseded: false,
            metadata: HashMap::new(),
        };
        let r = resolve_temporal_result(&[t], "-1", false);
        assert_eq!(r.as_deref(), Some("v1"));
    }

    #[test]
    fn resolve_mid_index() {
        let mk = |v: &str| Transition {
            value: v.into(),
            timestamp: String::new(),
            temporal_index: 0,
            experience_id: String::new(),
            sequence_position: 0,
            superseded: false,
            metadata: HashMap::new(),
        };
        let ts = vec![mk("a"), mk("b"), mk("c")];
        let r = resolve_temporal_result(&ts, "mid", false);
        assert_eq!(r.as_deref(), Some("b"));
    }
}
