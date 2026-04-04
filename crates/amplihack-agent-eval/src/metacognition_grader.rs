//! Metacognition grader with 4-dimension scoring.
//!
//! Ports Python `amplihack/eval/metacognition_grader.py`:
//! - 4 dimensions: factual_accuracy, self_awareness, knowledge_boundaries, explanation_quality
//! - Deterministic heuristic grading (no LLM calls)
//! - Batch grading support
//! - ReasoningTraceScore bridge for progressive test suite

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The four metacognition dimension names.
pub const DIMENSION_NAMES: [&str; 4] = [
    "factual_accuracy",
    "self_awareness",
    "knowledge_boundaries",
    "explanation_quality",
];

/// A single scoring dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    pub name: String,
    pub score: f64,
    pub reasoning: String,
}

/// Complete metacognition evaluation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetacognitionScore {
    pub dimensions: Vec<Dimension>,
    pub overall_score: f64,
    pub summary: String,
}

/// Grades student metacognition across 4 dimensions.
///
/// Uses deterministic heuristics (word overlap, explanation length, hedge
/// detection) rather than LLM calls.
#[derive(Debug, Clone)]
pub struct MetacognitionGrader {
    pub model: String,
}

impl Default for MetacognitionGrader {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-6".to_string(),
        }
    }
}

impl MetacognitionGrader {
    pub fn new(model: impl Into<String>) -> Self {
        let m = model.into();
        Self {
            model: if m.is_empty() {
                "claude-opus-4-6".to_string()
            } else {
                m
            },
        }
    }

    /// Grade a single question-answer pair on 4 metacognition dimensions.
    pub fn grade(
        &self,
        question: &str,
        expected_answer: &str,
        student_answer: &str,
        self_explanation: &str,
    ) -> MetacognitionScore {
        let factual = self.grade_factual_accuracy(expected_answer, student_answer);
        let awareness = self.grade_self_awareness(student_answer, self_explanation);
        let boundaries = self.grade_knowledge_boundaries(self_explanation);
        let quality = self.grade_explanation_quality(self_explanation, question);

        let dimensions = vec![factual, awareness, boundaries, quality];
        let overall = dimensions.iter().map(|d| d.score).sum::<f64>() / dimensions.len() as f64;
        let summary = Self::generate_summary(&dimensions, overall);

        MetacognitionScore {
            dimensions,
            overall_score: overall,
            summary,
        }
    }

    /// Grade multiple question-answer pairs.
    pub fn batch_grade(&self, items: &[GradeItem]) -> Vec<MetacognitionScore> {
        items
            .iter()
            .map(|item| {
                self.grade(
                    &item.question,
                    &item.expected,
                    &item.actual,
                    &item.explanation,
                )
            })
            .collect()
    }

    fn grade_factual_accuracy(&self, expected: &str, actual: &str) -> Dimension {
        let expected_words: Vec<&str> = expected
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|w| w.len() > 2)
            .collect();
        let actual_lower = actual.to_lowercase();

        if expected_words.is_empty() {
            return Dimension {
                name: "factual_accuracy".to_string(),
                score: 0.0,
                reasoning: "No expected content to compare".to_string(),
            };
        }

        let matches = expected_words
            .iter()
            .filter(|w| actual_lower.contains(&w.to_lowercase()))
            .count();
        let ratio = matches as f64 / expected_words.len() as f64;
        let score = ratio.min(1.0);

        let reasoning = if score >= 0.8 {
            format!(
                "High factual overlap ({matches}/{} key terms)",
                expected_words.len()
            )
        } else if score >= 0.5 {
            format!(
                "Moderate factual overlap ({matches}/{} key terms)",
                expected_words.len()
            )
        } else {
            format!(
                "Low factual overlap ({matches}/{} key terms)",
                expected_words.len()
            )
        };

        Dimension {
            name: "factual_accuracy".to_string(),
            score,
            reasoning,
        }
    }

    fn grade_self_awareness(&self, student_answer: &str, self_explanation: &str) -> Dimension {
        let mut score: f64 = 0.0;
        let mut reasons = Vec::new();
        let expl_lower = self_explanation.to_lowercase();

        // Has self-explanation at all
        if self_explanation.trim().len() > 10 {
            score += 0.3;
            reasons.push("Provides self-explanation");
        }

        // Shows awareness of what they know
        let confidence_markers = ["I know", "I think", "I believe", "I'm sure", "confident"];
        if confidence_markers
            .iter()
            .any(|m| expl_lower.contains(&m.to_lowercase()))
        {
            score += 0.35;
            reasons.push("Shows knowledge awareness");
        }

        // Shows awareness of what they don't know
        let uncertainty_markers = [
            "not sure",
            "don't know",
            "uncertain",
            "might be",
            "possibly",
            "I'm unsure",
            "unclear",
        ];
        if uncertainty_markers
            .iter()
            .any(|m| expl_lower.contains(&m.to_lowercase()))
        {
            score += 0.35;
            reasons.push("Acknowledges uncertainty");
        }

        // Bonus for substantive answer
        if student_answer.split_whitespace().count() >= 20 {
            score = (score + 0.1).min(1.0);
        }

        Dimension {
            name: "self_awareness".to_string(),
            score: score.min(1.0),
            reasoning: if reasons.is_empty() {
                "No self-awareness indicators".to_string()
            } else {
                reasons.join(" | ")
            },
        }
    }

    fn grade_knowledge_boundaries(&self, self_explanation: &str) -> Dimension {
        let mut score: f64 = 0.0;
        let mut reasons = Vec::new();
        let expl_lower = self_explanation.to_lowercase();

        // Explicit boundary markers
        let boundary_markers = [
            "I know that",
            "I don't know",
            "beyond my",
            "outside of",
            "not covered",
            "wasn't taught",
            "gap in",
            "limit",
        ];
        let boundary_count = boundary_markers
            .iter()
            .filter(|m| expl_lower.contains(&m.to_lowercase()))
            .count();

        if boundary_count >= 2 {
            score += 0.5;
            reasons.push(format!("{boundary_count} boundary markers"));
        } else if boundary_count == 1 {
            score += 0.3;
            reasons.push("1 boundary marker".to_string());
        }

        // Distinguishes known from unknown
        let has_known = expl_lower.contains("know") || expl_lower.contains("learned");
        let has_unknown = expl_lower.contains("don't")
            || expl_lower.contains("not sure")
            || expl_lower.contains("unclear");
        if has_known && has_unknown {
            score += 0.5;
            reasons.push("Distinguishes known from unknown".to_string());
        } else if has_known || has_unknown {
            score += 0.2;
        }

        Dimension {
            name: "knowledge_boundaries".to_string(),
            score: score.min(1.0),
            reasoning: if reasons.is_empty() {
                "No boundary identification".to_string()
            } else {
                reasons.join(" | ")
            },
        }
    }

    fn grade_explanation_quality(&self, self_explanation: &str, _question: &str) -> Dimension {
        let mut score: f64 = 0.0;
        let mut reasons = Vec::new();

        let words: Vec<&str> = self_explanation.split_whitespace().collect();
        let word_count = words.len();

        // Length check
        if word_count >= 30 {
            score += 0.3;
            reasons.push(format!("Substantive ({word_count} words)"));
        } else if word_count >= 15 {
            score += 0.2;
            reasons.push(format!("Moderate ({word_count} words)"));
        } else if word_count > 0 {
            score += 0.1;
        }

        let expl_lower = self_explanation.to_lowercase();

        // Reasoning markers
        let reasoning_markers = [
            "because",
            "therefore",
            "since",
            "due to",
            "as a result",
            "this means",
            "which shows",
            "indicates",
        ];
        let reasoning_count = reasoning_markers
            .iter()
            .filter(|m| expl_lower.contains(*m))
            .count();
        if reasoning_count >= 2 {
            score += 0.4;
            reasons.push(format!("{reasoning_count} reasoning markers"));
        } else if reasoning_count == 1 {
            score += 0.25;
            reasons.push("1 reasoning marker".to_string());
        }

        // Structure check
        let has_structure = self_explanation.contains("1.")
            || self_explanation.contains("- ")
            || self_explanation.contains("First")
            || self_explanation.contains("Second");
        if has_structure {
            score += 0.3;
            reasons.push("Structured reasoning".to_string());
        }

        Dimension {
            name: "explanation_quality".to_string(),
            score: score.min(1.0),
            reasoning: if reasons.is_empty() {
                "No meaningful explanation".to_string()
            } else {
                reasons.join(" | ")
            },
        }
    }

    fn generate_summary(dimensions: &[Dimension], overall: f64) -> String {
        let level = if overall >= 0.8 {
            "strong metacognition"
        } else if overall >= 0.6 {
            "moderate metacognition"
        } else if overall >= 0.4 {
            "limited metacognition"
        } else {
            "weak metacognition"
        };

        let strongest = dimensions
            .iter()
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
            .unwrap();
        let weakest = dimensions
            .iter()
            .min_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
            .unwrap();

        format!(
            "Student demonstrated {level} (overall: {overall:.2}). \
             Strongest: {} ({:.2}). Weakest: {} ({:.2}).",
            strongest.name, strongest.score, weakest.name, weakest.score
        )
    }

    pub fn zero_score(reason: &str) -> MetacognitionScore {
        let dimensions = DIMENSION_NAMES
            .iter()
            .map(|name| Dimension {
                name: name.to_string(),
                score: 0.0,
                reasoning: reason.to_string(),
            })
            .collect();
        MetacognitionScore {
            dimensions,
            overall_score: 0.0,
            summary: format!("Grading failed: {reason}"),
        }
    }
}

/// Input item for batch grading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeItem {
    pub question: String,
    pub expected: String,
    pub actual: String,
    #[serde(default)]
    pub explanation: String,
}

/// Score from reasoning trace analysis (progressive test suite interface).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningTraceScore {
    pub effort_calibration: f64,
    pub sufficiency_judgment: f64,
    pub search_quality: f64,
    pub self_correction: f64,
    pub overall: f64,
    pub details: HashMap<String, serde_json::Value>,
}

/// Grade metacognition from a reasoning trace (convenience function).
pub fn grade_metacognition(trace: &str, answer_score: f64, level: &str) -> ReasoningTraceScore {
    let grader = MetacognitionGrader::default();
    let truncated: String = trace.chars().take(2000).collect();

    let result = grader.grade(
        &format!("[{level}] Reasoning trace evaluation"),
        &format!("Score: {answer_score:.2}"),
        &truncated,
        &truncated,
    );

    let dim_map: HashMap<String, f64> = result
        .dimensions
        .iter()
        .map(|d| (d.name.clone(), d.score))
        .collect();

    let mut details = HashMap::new();
    let dim_details: HashMap<String, serde_json::Value> = result
        .dimensions
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                serde_json::json!({
                    "score": d.score,
                    "reasoning": d.reasoning,
                }),
            )
        })
        .collect();
    details.insert(
        "dimensions".to_string(),
        serde_json::to_value(&dim_details).unwrap_or_default(),
    );
    details.insert(
        "summary".to_string(),
        serde_json::Value::String(result.summary),
    );

    ReasoningTraceScore {
        effort_calibration: *dim_map.get("self_awareness").unwrap_or(&0.0),
        sufficiency_judgment: *dim_map.get("knowledge_boundaries").unwrap_or(&0.0),
        search_quality: *dim_map.get("factual_accuracy").unwrap_or(&0.0),
        self_correction: *dim_map.get("explanation_quality").unwrap_or(&0.0),
        overall: result.overall_score,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grader() {
        let g = MetacognitionGrader::default();
        assert!(g.model.contains("claude"));
    }

    #[test]
    fn grade_perfect_answer() {
        let g = MetacognitionGrader::default();
        let score = g.grade(
            "What does L1 evaluate?",
            "L1 evaluates direct recall of facts",
            "L1 evaluates direct recall of facts from articles",
            "I know this because L1 recall means remembering exact facts. \
             I'm not sure about edge cases though.",
        );
        assert!(score.overall_score > 0.3);
        assert_eq!(score.dimensions.len(), 4);
        assert!(!score.summary.is_empty());
    }

    #[test]
    fn grade_empty_answer() {
        let g = MetacognitionGrader::default();
        let score = g.grade("What is X?", "X is Y", "", "");
        assert!(score.overall_score < 0.2);
    }

    #[test]
    fn batch_grade_works() {
        let g = MetacognitionGrader::default();
        let items = vec![
            GradeItem {
                question: "Q1".to_string(),
                expected: "A1 with detail".to_string(),
                actual: "A1 with detail".to_string(),
                explanation: "Because I learned A1".to_string(),
            },
            GradeItem {
                question: "Q2".to_string(),
                expected: "A2".to_string(),
                actual: "wrong".to_string(),
                explanation: "".to_string(),
            },
        ];
        let results = g.batch_grade(&items);
        assert_eq!(results.len(), 2);
        assert!(results[0].overall_score >= results[1].overall_score);
    }

    #[test]
    fn zero_score_all_dimensions() {
        let score = MetacognitionGrader::zero_score("test error");
        assert_eq!(score.dimensions.len(), 4);
        assert!((score.overall_score).abs() < f64::EPSILON);
        assert!(score.summary.contains("test error"));
    }

    #[test]
    fn grade_metacognition_convenience() {
        let result = grade_metacognition(
            "I searched memory and found the answer because the fact was stored",
            0.8,
            "L2",
        );
        assert!(result.overall >= 0.0);
        assert!(result.details.contains_key("dimensions"));
        assert!(result.details.contains_key("summary"));
    }

    #[test]
    fn summary_levels() {
        let g = MetacognitionGrader::default();

        let high = g.grade(
            "Q",
            "answer with many key terms here",
            "answer with many key terms here present",
            "I know this because of reasoning. Therefore the answer \
             is clear. I don't know about other aspects though. \
             First, the main point. Second, the detail.",
        );
        assert!(high.summary.contains("metacognition"));

        let low = g.grade("Q", "answer", "", "");
        assert!(low.summary.contains("weak"));
    }

    #[test]
    fn reasoning_trace_score_serde() {
        let score = ReasoningTraceScore {
            effort_calibration: 0.5,
            sufficiency_judgment: 0.6,
            search_quality: 0.7,
            self_correction: 0.8,
            overall: 0.65,
            details: HashMap::new(),
        };
        let json = serde_json::to_string(&score).unwrap();
        let back: ReasoningTraceScore = serde_json::from_str(&json).unwrap();
        assert!((back.overall - 0.65).abs() < f64::EPSILON);
    }

    #[test]
    fn dimension_names_constant() {
        assert_eq!(DIMENSION_NAMES.len(), 4);
        assert!(DIMENSION_NAMES.contains(&"factual_accuracy"));
        assert!(DIMENSION_NAMES.contains(&"explanation_quality"));
    }
}
