//! Temporal state tracking, transition chains, and chronological reasoning.
//!
//! Port of Python `temporal_reasoning.py` — builds ordered transition chains
//! from facts, parses temporal index expressions from questions, and formats
//! deterministic temporal-lookup answers.

mod transitions;

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;

pub use transitions::{
    Transition, collapse_change_count_transitions, collapse_temporal_lookup_transitions,
    transition_chain_from_facts,
};

// ---------------------------------------------------------------------------
// Compiled regexes
// ---------------------------------------------------------------------------

pub(crate) static DATE_VALUE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\b((?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{1,2}(?:,\s+\d{4})?)\b",
    )
    .unwrap()
});

pub(crate) static FROM_TO_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bfrom\s+(.+?)\s+to\s+(.+)").unwrap());

pub(crate) static DEADLINE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)\b(?:current|latest|final|last|original|initial)\s+deadline\s+(?:is|was|of)\s+(?P<fragment>.+)$").unwrap(),
        Regex::new(r"(?i)\bdeadline\s+(?:is|was|of)\s+(?P<fragment>.+)$").unwrap(),
        Regex::new(r"(?i)\bdeadline\s+(?:has\s+been\s+)?(?:changed|moved|pushed|extended|updated)(?:\s+again)?\s+to\s+(?P<fragment>.+)$").unwrap(),
        Regex::new(r"(?i)\btarget\s+(?:delivery\s+)?date\s+(?:is|was|of)\s+(?P<fragment>.+)$").unwrap(),
    ]
});

static DIRECT_LOOKUP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^\s*(?:what|who)\s+(?:is|was)\s+the\s+(?P<qualifier>current|latest|original|initial|previous|final|last)\s+(?P<field>.+?)\s+(?:for|of)\s+(?P<entity>.+?)\s*\??\s*$",
    )
    .unwrap()
});

static ENTITY_TRAIL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\s+\b(?:before|after|when|as of)\b.*$").unwrap());

static AFTER_BEFORE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)after\s+(?:the\s+)?first.*?(?:but\s+)?before\s+(?:the\s+)?(?:second|final|last)",
    )
    .unwrap()
});

static BEFORE_FIRST_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)before\s+(?:the\s+)?(?:first|any)\s+(?:change|update|modification)").unwrap()
});

static AFTER_NTH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)after\s+(?:the\s+)?(\w+)\s+(?:change|update)").unwrap());

static BEFORE_FINAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)before\s+(?:the\s+)?(?:final|last|latest)\s+(?:change|update|value)").unwrap()
});

// ---------------------------------------------------------------------------
// Temporal keyword map
// ---------------------------------------------------------------------------

fn temporal_keyword_index(kw: &str) -> Option<&'static str> {
    match kw {
        "first" | "original" | "initial" => Some("0"),
        "second" => Some("1"),
        "third" => Some("2"),
        "intermediate" | "middle" | "between" => Some("mid"),
        "latest" | "current" | "final" | "last" => Some("-1"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public API — value extraction
// ---------------------------------------------------------------------------

/// Extract temporal date-like values from a text for a given field.
pub fn extract_temporal_state_values(value: &str, field: &str) -> Vec<String> {
    let cleaned = value.replace('*', "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return Vec::new();
    }
    let field_lower = field.to_lowercase();

    if field_lower == "date" || field_lower == "deadline" {
        if let Some(cap) = FROM_TO_RE.captures(cleaned) {
            let sequence = &cleaned[cap.get(0).unwrap().start()..];
            let sequence = Regex::new(r"(?i)^from\s+").unwrap().replace(sequence, "");
            let parts: Vec<&str> = Regex::new(r"(?i)\s+to\s+")
                .unwrap()
                .split(&sequence)
                .collect();
            let mut ordered = Vec::new();
            let mut seen = HashSet::new();
            for part in parts {
                if let Some(m) = DATE_VALUE_RE.find(part) {
                    let key = m.as_str().to_lowercase();
                    if seen.insert(key) {
                        ordered.push(m.as_str().to_string());
                    }
                }
            }
            if ordered.len() > 1 {
                return ordered;
            }
        }

        if field_lower == "deadline" {
            for pat in DEADLINE_PATTERNS.iter() {
                if let Some(cap) = pat.captures(cleaned)
                    && let Some(frag) = cap.name("fragment")
                    && let Some(m) = DATE_VALUE_RE.find(frag.as_str())
                {
                    return vec![m.as_str().to_string()];
                }
            }
        }

        let matches: Vec<String> = DATE_VALUE_RE
            .find_iter(cleaned)
            .map(|m| m.as_str().to_string())
            .collect();
        if matches.len() == 1 {
            return matches;
        }
        return Vec::new();
    }

    vec![cleaned.to_string()]
}

// ---------------------------------------------------------------------------
// Public API — temporal index parsing
// ---------------------------------------------------------------------------

/// Parse a temporal question to determine which state index expression to use.
pub fn parse_temporal_index(question: &str) -> String {
    let lower = question.to_lowercase();

    if AFTER_BEFORE_RE.is_match(&lower) {
        return "1".into();
    }
    if BEFORE_FIRST_RE.is_match(&lower) {
        return "0".into();
    }
    if let Some(cap) = AFTER_NTH_RE.captures(&lower) {
        let ordinal = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        match ordinal {
            "first" => return "1".into(),
            "second" => return "2".into(),
            "third" => return "3".into(),
            _ => {}
        }
    }
    if BEFORE_FINAL_RE.is_match(&lower) {
        return "-2".into();
    }
    for kw in [
        "first",
        "original",
        "initial",
        "second",
        "third",
        "intermediate",
        "middle",
        "between",
        "latest",
        "current",
        "final",
        "last",
    ] {
        if lower.contains(kw)
            && let Some(expr) = temporal_keyword_index(kw)
        {
            return expr.to_string();
        }
    }
    "-1".into()
}

/// Heuristic extraction of entity/field from a direct temporal lookup question.
pub fn heuristic_temporal_entity_field(question: &str) -> Option<(String, String)> {
    let cap = DIRECT_LOOKUP_RE.captures(question.trim())?;
    let field = cap
        .name("field")
        .map(|m| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))?;
    let field = field.trim_matches(|c: char| " .?!".contains(c)).to_string();
    let raw_entity = cap.name("entity")?.as_str();
    let entity = ENTITY_TRAIL_RE.replace(raw_entity, "").to_string();
    let entity = entity
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| " .?!".contains(c))
        .to_string();
    if entity.is_empty() || field.is_empty() {
        return None;
    }
    Some((entity, field))
}

/// Whether a temporal code result should be short-circuited.
pub fn should_short_circuit_temporal_answer(
    question: &str,
    result: Option<&str>,
    operation: Option<&str>,
) -> bool {
    if result.is_none() {
        return false;
    }
    if operation != Some("state_lookup") {
        return false;
    }
    DIRECT_LOOKUP_RE.is_match(question.trim())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amplihack_memory::Fact;
    use std::collections::HashMap;

    #[test]
    fn extract_date_values_single() {
        let vals = extract_temporal_state_values("deadline is March 15, 2024", "deadline");
        assert_eq!(vals, vec!["March 15, 2024"]);
    }

    #[test]
    fn extract_date_values_from_to() {
        let vals = extract_temporal_state_values(
            "changed from March 15, 2024 to April 1, 2024",
            "deadline",
        );
        assert_eq!(vals.len(), 2);
        assert!(vals[0].contains("March"));
        assert!(vals[1].contains("April"));
    }

    #[test]
    fn extract_non_date_field() {
        let vals = extract_temporal_state_values("Alice Smith", "manager");
        assert_eq!(vals, vec!["Alice Smith"]);
    }

    #[test]
    fn parse_index_latest() {
        assert_eq!(parse_temporal_index("What is the current deadline?"), "-1");
    }

    #[test]
    fn parse_index_original() {
        assert_eq!(parse_temporal_index("What was the original deadline?"), "0");
    }

    #[test]
    fn parse_index_after_first() {
        assert_eq!(
            parse_temporal_index("What was it after the first change?"),
            "1"
        );
    }

    #[test]
    fn parse_index_before_final() {
        assert_eq!(
            parse_temporal_index("What was the value before the final change?"),
            "-2"
        );
    }

    #[test]
    fn heuristic_entity_field_basic() {
        let q = "What is the current deadline for Atlas project?";
        let (entity, field) = heuristic_temporal_entity_field(q).unwrap();
        assert_eq!(entity, "Atlas project");
        assert_eq!(field, "deadline");
    }

    #[test]
    fn heuristic_entity_field_none() {
        assert!(heuristic_temporal_entity_field("Hello world").is_none());
    }

    #[test]
    fn transition_chain_basic() {
        let facts = vec![
            Fact::new(
                "Atlas deadline",
                "deadline changed from March 15 to April 1",
            ),
            Fact::new("Atlas deadline", "Project deadline is April 1"),
        ];
        let chain = transition_chain_from_facts("Atlas", "deadline", &facts);
        assert!(!chain.is_empty());
    }

    #[test]
    fn collapse_change_count() {
        let t1 = Transition {
            value: "March 15".into(),
            timestamp: "t1".into(),
            temporal_index: 0,
            experience_id: "e1".into(),
            sequence_position: 0,
            superseded: true,
            metadata: HashMap::new(),
        };
        let t2 = Transition {
            value: "March 15".into(),
            timestamp: "t2".into(),
            temporal_index: 1,
            experience_id: "e2".into(),
            sequence_position: 0,
            superseded: false,
            metadata: HashMap::new(),
        };
        let collapsed = collapse_change_count_transitions(&[t1, t2], "deadline");
        assert_eq!(collapsed.len(), 1);
    }

    #[test]
    fn short_circuit_check() {
        assert!(should_short_circuit_temporal_answer(
            "What is the current deadline for Atlas?",
            Some("March 15"),
            Some("state_lookup"),
        ));
        assert!(!should_short_circuit_temporal_answer(
            "How many times changed?",
            Some("3"),
            Some("change_count"),
        ));
    }
}
