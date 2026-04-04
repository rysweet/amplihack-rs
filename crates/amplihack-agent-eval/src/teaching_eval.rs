//! Teaching evaluation layer for domain agents.
//!
//! Ports Python `amplihack/eval/teaching_eval.py`:
//! - 4 grading dimensions: clarity, completeness, student_performance, adaptivity
//! - Deterministic heuristic grading (structure, examples, word count, domain terms)
//! - Combined domain + teaching evaluation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Score for a single teaching dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachingDimensionScore {
    pub dimension: String,
    pub score: f64,
    pub weight: f64,
    pub details: String,
}

impl TeachingDimensionScore {
    pub fn new(
        dimension: impl Into<String>,
        score: f64,
        weight: f64,
        details: impl Into<String>,
    ) -> Self {
        Self {
            dimension: dimension.into(),
            score: score.min(1.0),
            weight,
            details: details.into(),
        }
    }
}

/// Raw result from an agent's teach() method.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeachingResult {
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub lesson_plan: String,
    #[serde(default)]
    pub agent_answers: Vec<String>,
    #[serde(default)]
    pub student_attempt: String,
}

/// Complete teaching evaluation result for a domain agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachingEvalResult {
    pub agent_name: String,
    pub domain: String,
    pub topic: String,
    pub student_level: String,
    pub dimension_scores: Vec<TeachingDimensionScore>,
    pub composite_score: f64,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TeachingEvalResult {
    /// Recompute composite score from dimension weights.
    pub fn recompute_composite(&mut self) {
        self.composite_score = self
            .dimension_scores
            .iter()
            .map(|d| d.score * d.weight)
            .sum();
    }

    /// Serialize to a summary dict.
    pub fn to_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "agent_name": self.agent_name,
            "domain": self.domain,
            "topic": self.topic,
            "student_level": self.student_level,
            "composite_score": (self.composite_score * 1000.0).round() / 1000.0,
            "dimensions": self.dimension_scores.iter().map(|d| {
                serde_json::json!({
                    "dimension": d.dimension,
                    "score": (d.score * 1000.0).round() / 1000.0,
                    "weight": d.weight,
                    "details": d.details,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Dimension weights
// ---------------------------------------------------------------------------

/// Default dimension weights matching the Python implementation.
pub fn default_weights() -> HashMap<&'static str, f64> {
    let mut w = HashMap::new();
    w.insert("clarity", 0.25);
    w.insert("completeness", 0.25);
    w.insert("student_performance", 0.30);
    w.insert("adaptivity", 0.20);
    w
}

// ---------------------------------------------------------------------------
// Grading functions
// ---------------------------------------------------------------------------

/// Grade the clarity of instruction.
pub fn grade_clarity(result: &TeachingResult, domain: &str) -> TeachingDimensionScore {
    let weight = 0.25;
    let instruction = &result.instruction;

    if instruction.trim().is_empty() {
        return TeachingDimensionScore::new("clarity", 0.0, weight, "No instruction provided");
    }

    let mut score = 0.0_f64;
    let mut details = Vec::new();

    // Length check
    let words: Vec<&str> = instruction.split_whitespace().collect();
    if words.len() >= 50 {
        score += 0.25;
        details.push(format!("Sufficient length ({} words)", words.len()));
    } else if words.len() >= 20 {
        score += 0.15;
        details.push(format!("Moderate length ({} words)", words.len()));
    } else {
        details.push(format!("Too short ({} words)", words.len()));
    }

    // Structure check
    let has_structure = ["1.", "2.", "- ", "**"]
        .iter()
        .any(|m| instruction.contains(m));
    if has_structure {
        score += 0.25;
        details.push("Has structure".into());
    }

    // Example check
    let has_examples = ["example", "for instance", "bad:", "good:", "e.g."]
        .iter()
        .any(|m| instruction.to_lowercase().contains(m));
    if has_examples {
        score += 0.25;
        details.push("Includes examples".into());
    }

    // Domain terms check
    let domain_terms = get_domain_terms(domain);
    let terms_found = domain_terms
        .iter()
        .filter(|t| instruction.to_lowercase().contains(&t.to_lowercase()))
        .count();
    if terms_found >= 3 {
        score += 0.25;
        details.push(format!("Uses {terms_found} domain terms"));
    } else if terms_found >= 1 {
        score += 0.15;
        details.push(format!("Uses {terms_found} domain terms"));
    }

    TeachingDimensionScore::new("clarity", score.min(1.0), weight, details.join(" | "))
}

/// Grade the completeness of teaching.
pub fn grade_completeness(result: &TeachingResult) -> TeachingDimensionScore {
    let weight = 0.25;
    let mut score = 0.0_f64;
    let mut details = Vec::new();

    // Lesson plan quality
    if result.lesson_plan.trim().len() > 20 {
        let plan_items: Vec<&str> = result
            .lesson_plan
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        if plan_items.len() >= 4 {
            score += 0.3;
            details.push(format!("Lesson plan: {} items", plan_items.len()));
        } else if plan_items.len() >= 2 {
            score += 0.2;
            details.push(format!("Lesson plan: {} items", plan_items.len()));
        }
    } else {
        details.push("Weak lesson plan".into());
    }

    // Multi-section instruction
    if !result.instruction.is_empty() {
        let sections = result.instruction.matches("\n\n").count();
        if sections >= 3 {
            score += 0.25;
            details.push(format!("{} instruction sections", sections + 1));
        } else if sections >= 1 {
            score += 0.15;
        }
    }

    // Answers provided
    if !result.agent_answers.is_empty() {
        let substantive = result.agent_answers.iter().filter(|a| a.len() > 20).count();
        if substantive >= 2 {
            score += 0.25;
            details.push(format!("{substantive} substantive answers"));
        } else if substantive >= 1 {
            score += 0.15;
        }
    }

    // Practice material
    if result.student_attempt.trim().len() > 20 {
        score += 0.2;
        details.push("Practice material present".into());
    }

    TeachingDimensionScore::new("completeness", score.min(1.0), weight, details.join(" | "))
}

/// Grade student performance on practice.
pub fn grade_student_performance(result: &TeachingResult) -> TeachingDimensionScore {
    let weight = 0.30;
    let attempt = &result.student_attempt;

    if attempt.trim().is_empty() {
        return TeachingDimensionScore::new(
            "student_performance",
            0.0,
            weight,
            "No student attempt",
        );
    }

    let mut score = 0.0_f64;
    let mut details = Vec::new();

    let words: Vec<&str> = attempt.split_whitespace().collect();
    if words.len() >= 30 {
        score += 0.3;
        details.push(format!("Substantive attempt ({} words)", words.len()));
    } else if words.len() >= 15 {
        score += 0.2;
    }

    // Finding indicators
    let finding_markers = [
        "found",
        "identified",
        "detected",
        "issue",
        "finding",
        "action",
    ];
    let has_findings = finding_markers
        .iter()
        .any(|m| attempt.to_lowercase().contains(m));
    if has_findings {
        score += 0.35;
        details.push("Shows findings".into());
    }

    // Structure indicators
    let structure_markers = ["- ", "* ", "1.", ":", "Summary", "Action"];
    let has_structure = structure_markers.iter().any(|m| attempt.contains(m));
    if has_structure {
        score += 0.35;
        details.push("Structured output".into());
    }

    TeachingDimensionScore::new(
        "student_performance",
        score.min(1.0),
        weight,
        details.join(" | "),
    )
}

/// Grade agent's adaptivity to student needs.
pub fn grade_adaptivity(result: &TeachingResult) -> TeachingDimensionScore {
    let weight = 0.20;
    let mut score = 0.0_f64;
    let mut details = Vec::new();

    // Varied responses
    if result.agent_answers.len() >= 2 && result.agent_answers[0] != result.agent_answers[1] {
        score += 0.35;
        details.push("Varied responses".into());
    }

    // Answer quality
    if !result.agent_answers.is_empty() {
        let avg_len: f64 = result
            .agent_answers
            .iter()
            .map(|a| a.len() as f64)
            .sum::<f64>()
            / result.agent_answers.len() as f64;
        if avg_len > 100.0 {
            score += 0.3;
            details.push(format!("Detailed answers (avg {avg_len:.0} chars)"));
        } else if avg_len > 50.0 {
            score += 0.2;
        }
    }

    // Level awareness
    let level_words = ["beginner", "intermediate", "advanced", "student level"];
    if !result.lesson_plan.is_empty()
        && level_words
            .iter()
            .any(|w| result.lesson_plan.to_lowercase().contains(w))
    {
        score += 0.35;
        details.push("Level-aware".into());
    }

    TeachingDimensionScore::new("adaptivity", score.min(1.0), weight, details.join(" | "))
}

/// Run a complete teaching evaluation.
pub fn evaluate_teaching(
    agent_name: &str,
    domain: &str,
    topic: &str,
    student_level: &str,
    result: &TeachingResult,
) -> TeachingEvalResult {
    let dimension_scores = vec![
        grade_clarity(result, domain),
        grade_completeness(result),
        grade_student_performance(result),
        grade_adaptivity(result),
    ];

    let composite: f64 = dimension_scores.iter().map(|d| d.score * d.weight).sum();

    TeachingEvalResult {
        agent_name: agent_name.to_string(),
        domain: domain.to_string(),
        topic: topic.to_string(),
        student_level: student_level.to_string(),
        dimension_scores,
        composite_score: composite,
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Domain term lookup
// ---------------------------------------------------------------------------

/// Get domain-specific terms for instruction quality checking.
pub fn get_domain_terms(domain: &str) -> Vec<&'static str> {
    match domain {
        "code_review" => vec![
            "bug",
            "security",
            "vulnerability",
            "style",
            "naming",
            "convention",
            "injection",
            "refactor",
            "test",
            "pattern",
        ],
        "meeting_synthesizer" => vec![
            "action item",
            "decision",
            "speaker",
            "transcript",
            "summary",
            "deadline",
            "owner",
            "follow-up",
        ],
        "document_creator" => vec!["template", "format", "section", "outline", "audience"],
        "data_analysis" => vec!["statistics", "trend", "correlation", "dataset", "insight"],
        "project_planning" => vec!["task", "milestone", "dependency", "risk", "timeline"],
        _ => vec![],
    }
}

/// Compute a combined domain + teaching score.
pub fn combined_score(
    domain_score: f64,
    teaching_score: f64,
    domain_weight: f64,
    teaching_weight: f64,
) -> f64 {
    domain_score * domain_weight + teaching_score * teaching_weight
}

#[cfg(test)]
#[path = "tests/teaching_eval_tests.rs"]
mod tests;
