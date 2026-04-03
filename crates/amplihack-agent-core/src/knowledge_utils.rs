//! Arithmetic validation, entity helpers, and fact extraction utilities.
//!
//! Port of Python `knowledge_utils.py` — provides person-name/project-name
//! detection, APT attribution helpers, arithmetic validation, and a trait
//! for LLM-driven fact extraction.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};

use amplihack_memory::Fact;

use crate::safe_calc::safe_eval;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PERSON_DETAIL_CUES: &[&str] = &[
    "allerg", "birthday", "degree", "favorite food", "hobby", "hometown",
    "personal information", "pet", "team",
];

static NON_PERSON_NAME_TOKENS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "affected", "allergies", "allergy", "award", "birthday", "center",
        "city", "climate", "competitor", "computer", "customer", "daily",
        "data", "database", "development", "difference", "discrepancy",
        "educational", "engineering", "estimate", "experience", "facilities",
        "favorite", "food", "framework", "gartner", "hiring", "hobby",
        "hometown", "incident", "indicators", "information", "innovation",
        "internal", "island", "market", "marketing", "migration",
        "nationality", "newsletter", "personnel", "personal", "pet", "pets",
        "preference", "product", "professional", "production", "project",
        "report", "rhode", "school", "security", "segment", "senior", "size",
        "sprint", "success", "satisfaction", "team", "threat", "user",
        "vulnerability",
    ]
    .into_iter()
    .collect()
});

static NON_PROJECT_NAMES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "assignment", "framework", "identity", "lead", "leadership",
        "management", "new", "overview", "status", "type",
    ]
    .into_iter()
    .collect()
});

static PERSON_NAME_PART_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Z][a-z]*(?:['\u{2019}\-][A-Z]?[a-z]+)?$").unwrap());

const PERSON_FRAG: &str =
    r"[A-Z][a-z]*(?:['\x{2019}\-][A-Z]?[a-z]+)?\s+[A-Z][a-z]*(?:['\x{2019}\-][A-Z]?[a-z]+)?";

static PERSON_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"\b({PERSON_FRAG})\b")).unwrap());

static PERSON_ATTR_POSSESSIVE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"\b({PERSON_FRAG})(?:'s|\x{{2019}}s)\s+(?:birthday|favorite food|degree|hobby|hometown|pet|team|allerg(?:y|ies)|nationality)\b"
    ))
    .unwrap()
});

static PERSON_ATTR_VERB_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"\b({PERSON_FRAG})\s+(?:has|holds|is)\b")).unwrap());

static PROJECT_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bProject\s+([A-Z][A-Za-z0-9_-]+)\b").unwrap());

static APT_NUM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bapt(?:-| )?\d+\b").unwrap());

const APT_ATTRIBUTION_FACT_CUES: &[&str] =
    &["apt", "attribution", "development infrastructure", "threat actor", "ttp"];

static ARITHMETIC_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(\d+(?:\.\d+)?)\s*([+\-*/])\s*(\d+(?:\.\d+)?)\s*=\s*(\d+(?:\.\d+)?)").unwrap()
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Format a list as "A", "A and B", or "A, B, and C".
pub fn format_distinct_item_list(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        _ => {
            let (head, tail) = items.split_at(items.len() - 1);
            format!("{}, and {}", head.join(", "), tail[0])
        }
    }
}

/// Normalize a person name by collapsing whitespace and stripping possessives.
pub fn normalize_person_name(candidate: &str) -> String {
    let trimmed: String = candidate.split_whitespace().collect::<Vec<_>>().join(" ");
    trimmed
        .replace("'s", "")
        .replace('\u{2019}', "")
        .replace("s ", " ")
        .trim()
        .to_string()
}

/// Check if a candidate string looks like a two-part person name.
pub fn looks_like_person_name(candidate: &str) -> bool {
    let norm = normalize_person_name(candidate);
    if norm.is_empty() || norm.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = norm.split_whitespace().collect();
    if parts.len() != 2 {
        return false;
    }
    parts.iter().all(|p| {
        PERSON_NAME_PART_RE.is_match(p)
            && !NON_PERSON_NAME_TOKENS.contains(&p.to_lowercase().as_str())
    })
}

/// Check if a candidate string looks like a project name.
pub fn looks_like_project_name(candidate: &str) -> bool {
    let cleaned = candidate.trim_matches(|c: char| ".,;:!?()[]{}\"'".contains(c));
    if cleaned.is_empty() || cleaned.contains(char::is_whitespace) {
        return false;
    }
    !NON_PROJECT_NAMES.contains(&cleaned.to_lowercase().as_str())
}

/// Extract person names mentioned alongside personal detail cues.
pub fn extract_personal_detail_people(facts: &[Fact]) -> Vec<String> {
    let mut people: HashMap<String, String> = HashMap::new();
    let mut personal_texts = Vec::new();

    for fact in facts {
        let text = format!("{} {}", fact.context, fact.outcome);
        let lower = text.to_lowercase();
        if !PERSON_DETAIL_CUES.iter().any(|c| lower.contains(c)) {
            continue;
        }
        personal_texts.push(text.clone());
        for re in [&*PERSON_ATTR_POSSESSIVE_RE, &*PERSON_ATTR_VERB_RE] {
            for cap in re.captures_iter(&text) {
                if let Some(m) = cap.get(1) {
                    let cleaned = normalize_person_name(m.as_str());
                    if looks_like_person_name(&cleaned) {
                        people.entry(cleaned.to_lowercase()).or_insert(cleaned);
                    }
                }
            }
        }
    }

    if people.is_empty() {
        for text in &personal_texts {
            for cap in PERSON_NAME_RE.captures_iter(text) {
                if let Some(m) = cap.get(1) {
                    let cleaned = normalize_person_name(m.as_str());
                    if looks_like_person_name(&cleaned) {
                        people.entry(cleaned.to_lowercase()).or_insert(cleaned);
                    }
                }
            }
        }
    }

    let mut keys: Vec<_> = people.keys().cloned().collect();
    keys.sort();
    keys.into_iter().map(|k| people[&k].clone()).collect()
}

/// Extract project names from facts (e.g. "Project Atlas").
pub fn extract_project_names(facts: &[Fact]) -> Vec<String> {
    let mut ctx: HashMap<String, String> = HashMap::new();
    let mut out: HashMap<String, String> = HashMap::new();
    let mut out_counts: HashMap<String, usize> = HashMap::new();

    for fact in facts {
        for cap in PROJECT_NAME_RE.captures_iter(&fact.context) {
            let name = cap[1].trim_matches(|c: char| ".,;:!?()[]{}\"'".contains(c));
            if looks_like_project_name(name) {
                ctx.entry(name.to_lowercase()).or_insert_with(|| name.to_string());
            }
        }
        for cap in PROJECT_NAME_RE.captures_iter(&fact.outcome) {
            let name = cap[1].trim_matches(|c: char| ".,;:!?()[]{}\"'".contains(c));
            if looks_like_project_name(name) {
                let key = name.to_lowercase();
                out.entry(key.clone()).or_insert_with(|| name.to_string());
                *out_counts.entry(key).or_insert(0) += 1;
            }
        }
    }

    let mut projects = ctx.clone();
    for (k, v) in &out {
        if projects.contains_key(k) || out_counts.get(k).copied().unwrap_or(0) >= 2 {
            projects.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut keys: Vec<_> = projects.keys().cloned().collect();
    keys.sort();
    keys.into_iter().map(|k| projects[&k].clone()).collect()
}

/// Whether any fact mentions a specific APT group number.
pub fn facts_contain_specific_apt(facts: &[Fact]) -> bool {
    facts.iter().any(|f| {
        let combined = format!("{} {}", f.context, f.outcome);
        APT_NUM_RE.is_match(&combined)
    })
}

/// Whether a question is asking about APT attribution.
pub fn is_apt_attribution_question(question: &str) -> bool {
    let lower = question.to_lowercase();
    lower.contains("apt")
        && ["attributed", "group", "threat actor"]
            .iter()
            .any(|c| lower.contains(c))
}

/// Validate arithmetic expressions in text using the safe calculator.
///
/// Finds patterns like `26 - 18 = 8` and corrects wrong results.
pub fn validate_arithmetic(answer: &str) -> String {
    let mut result = answer.to_string();
    for cap in ARITHMETIC_RE.captures_iter(answer) {
        let a = &cap[1];
        let op = &cap[2];
        let b = &cap[3];
        let claimed = &cap[4];
        let expr = format!("{a} {op} {b}");
        if let Ok(actual) = safe_eval(&expr)
            && let Ok(claimed_f) = claimed.parse::<f64>()
            && (actual - claimed_f).abs() > 0.01
        {
            let correct = if actual == actual.trunc() {
                format!("{}", actual as i64)
            } else {
                format!("{actual}")
            };
            let old = cap[0].to_string();
            let new = format!("{a} {op} {b} = {correct}");
            result = result.replacen(&old, &new, 1);
        }
    }
    result
}

/// Return the APT-attribution fact-cue keywords.
pub fn apt_attribution_fact_cues() -> &'static [&'static str] {
    APT_ATTRIBUTION_FACT_CUES
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_empty() {
        assert_eq!(format_distinct_item_list(&[]), "");
    }

    #[test]
    fn format_single() {
        assert_eq!(format_distinct_item_list(&["a".into()]), "a");
    }

    #[test]
    fn format_two() {
        let r = format_distinct_item_list(&["a".into(), "b".into()]);
        assert_eq!(r, "a and b");
    }

    #[test]
    fn format_three() {
        let r = format_distinct_item_list(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(r, "a, b, and c");
    }

    #[test]
    fn person_name_valid() {
        assert!(looks_like_person_name("John Smith"));
        assert!(looks_like_person_name("Mary O'Brien"));
    }

    #[test]
    fn person_name_invalid() {
        assert!(!looks_like_person_name("Project Atlas"));
        assert!(!looks_like_person_name("123 Street"));
        assert!(!looks_like_person_name("singleword"));
    }

    #[test]
    fn project_name_valid() {
        assert!(looks_like_project_name("Atlas"));
        assert!(looks_like_project_name("Phoenix-2"));
    }

    #[test]
    fn project_name_invalid() {
        assert!(!looks_like_project_name("management"));
        assert!(!looks_like_project_name("two words"));
    }

    #[test]
    fn extract_people_from_facts() {
        let facts = vec![Fact::new(
            "Personal information",
            "John Smith's birthday is in March",
        )];
        let people = extract_personal_detail_people(&facts);
        assert!(people.iter().any(|p| p.contains("John")));
    }

    #[test]
    fn extract_projects() {
        let facts = vec![
            Fact::new("Project Atlas deadline", "Due in March"),
            Fact::new("General", "Project Atlas is on track"),
        ];
        let names = extract_project_names(&facts);
        assert!(names.iter().any(|n| n == "Atlas"));
    }

    #[test]
    fn apt_detection() {
        let facts = vec![Fact::new("Threat", "APT-29 is responsible")];
        assert!(facts_contain_specific_apt(&facts));
    }

    #[test]
    fn apt_question() {
        assert!(is_apt_attribution_question(
            "Which APT group is attributed?"
        ));
        assert!(!is_apt_attribution_question("What is the weather?"));
    }

    #[test]
    fn arithmetic_validation_correct() {
        let s = validate_arithmetic("The answer is 10 + 5 = 15.");
        assert!(s.contains("= 15"));
    }

    #[test]
    fn arithmetic_validation_wrong() {
        let s = validate_arithmetic("Total: 26 - 18 = 10");
        assert!(s.contains("= 8"), "got: {s}");
    }
}
