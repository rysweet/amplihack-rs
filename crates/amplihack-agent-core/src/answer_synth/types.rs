//! Data types for OODA-based adaptive answer synthesis.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Complexity tier for a question (L1–L5, L11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuestionLevel {
    #[default]
    L1,
    L2,
    L3,
    L4,
    L5,
    L11,
}

impl fmt::Display for QuestionLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::L1 => write!(f, "L1"),
            Self::L2 => write!(f, "L2"),
            Self::L3 => write!(f, "L3"),
            Self::L4 => write!(f, "L4"),
            Self::L5 => write!(f, "L5"),
            Self::L11 => write!(f, "L11"),
        }
    }
}

impl QuestionLevel {
    pub fn parse(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "L1" => Self::L1,
            "L2" => Self::L2,
            "L3" => Self::L3,
            "L4" => Self::L4,
            "L5" => Self::L5,
            "L11" => Self::L11,
            _ => Self::L1,
        }
    }

    pub fn instruction(&self) -> &'static str {
        match self {
            Self::L1 => {
                "Provide a direct, factual answer. State it clearly and concisely. \
                          Do NOT add arithmetic verification — just report the facts as stored."
            }
            Self::L2 => "Connect multiple facts to infer an answer. Explain your reasoning.",
            Self::L3 => "Synthesize information from the facts to create a comprehensive answer.",
            Self::L4 => {
                "Apply the knowledge. For PROCEDURAL questions, reconstruct the exact \
                          ordered sequence of steps. Number each step. Answer ONLY what is asked."
            }
            Self::L5 => {
                "This question involves POTENTIALLY CONFLICTING information. \
                          Identify contradictory claims and present BOTH sides with sources."
            }
            Self::L11 => "Synthesize information from the facts to create a comprehensive answer.",
        }
    }
}

/// Detected intent classification for a question.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentType {
    #[default]
    SimpleRecall,
    MultiSourceSynthesis,
    TemporalComparison,
    MathematicalComputation,
    RatioTrendAnalysis,
    ContradictionResolution,
    MetaMemory,
    IncrementalUpdate,
    CausalCounterfactual,
}

impl fmt::Display for IntentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SimpleRecall => "simple_recall",
            Self::MultiSourceSynthesis => "multi_source_synthesis",
            Self::TemporalComparison => "temporal_comparison",
            Self::MathematicalComputation => "mathematical_computation",
            Self::RatioTrendAnalysis => "ratio_trend_analysis",
            Self::ContradictionResolution => "contradiction_resolution",
            Self::MetaMemory => "meta_memory",
            Self::IncrementalUpdate => "incremental_update",
            Self::CausalCounterfactual => "causal_counterfactual",
        })
    }
}

impl IntentType {
    pub fn parse(s: &str) -> Self {
        match s {
            "simple_recall" => Self::SimpleRecall,
            "multi_source_synthesis" => Self::MultiSourceSynthesis,
            "temporal_comparison" => Self::TemporalComparison,
            "mathematical_computation" => Self::MathematicalComputation,
            "ratio_trend_analysis" => Self::RatioTrendAnalysis,
            "contradiction_resolution" => Self::ContradictionResolution,
            "meta_memory" => Self::MetaMemory,
            "incremental_update" => Self::IncrementalUpdate,
            "causal_counterfactual" => Self::CausalCounterfactual,
            _ => Self::SimpleRecall,
        }
    }
    pub fn is_simple(&self) -> bool {
        matches!(self, Self::SimpleRecall | Self::IncrementalUpdate)
    }
    pub fn is_aggregation(&self) -> bool {
        matches!(self, Self::MetaMemory)
    }
    pub fn is_complex(&self) -> bool {
        !self.is_simple()
    }
}

/// Full intent classification result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectedIntent {
    pub intent: IntentType,
    pub needs_temporal: bool,
    pub needs_math: bool,
    #[serde(default)]
    pub computed_math: Option<String>,
    #[serde(default)]
    pub temporal_code: Option<TemporalCodeResult>,
    #[serde(default)]
    pub summary_context: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_specific_facts: Vec<HashMap<String, Value>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

/// Result from deterministic temporal code generation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemporalCodeResult {
    pub code: String,
    pub result: Option<Value>,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub state_count: usize,
    #[serde(default)]
    pub transitions: Vec<Value>,
}

/// Tuneable knobs for the synthesis pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisConfig {
    pub model: String,
    pub synthesis_temperature: f64,
    pub eval_temperature: f64,
    pub max_facts_complex: usize,
    pub max_facts_simple: usize,
    pub simple_retrieval_threshold: usize,
    pub max_retrieval_limit: usize,
    pub keyword_expansion_sparse_threshold: usize,
}

impl Default for SynthesisConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-6".to_string(),
            synthesis_temperature: 0.3,
            eval_temperature: 0.0,
            max_facts_complex: 500,
            max_facts_simple: 300,
            simple_retrieval_threshold: 100,
            max_retrieval_limit: 10_000,
            keyword_expansion_sparse_threshold: 30,
        }
    }
}

/// Result from evaluating whether an answer is complete.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletenessEvaluation {
    pub is_complete: bool,
    pub gaps: Vec<String>,
}

/// Keywords that force aggregation routing even if intent was misclassified.
pub const ENUMERATION_KEYWORDS: &[&str] = &[
    "list all",
    "which topics",
    "how many different",
    "enumerate",
    "what are all",
    "name all",
    "show all",
    "count all",
    "every incident",
    "all incidents",
    "all cve",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_level_roundtrip_and_instructions() {
        for level in [
            QuestionLevel::L1,
            QuestionLevel::L2,
            QuestionLevel::L3,
            QuestionLevel::L4,
            QuestionLevel::L5,
            QuestionLevel::L11,
        ] {
            assert_eq!(QuestionLevel::parse(&level.to_string()), level);
            assert!(!level.instruction().is_empty());
        }
        assert_eq!(QuestionLevel::parse("l3"), QuestionLevel::L3);
        assert_eq!(QuestionLevel::parse("L99"), QuestionLevel::L1);
    }

    #[test]
    fn intent_type_roundtrip() {
        let cases = [
            ("simple_recall", IntentType::SimpleRecall),
            ("multi_source_synthesis", IntentType::MultiSourceSynthesis),
            ("temporal_comparison", IntentType::TemporalComparison),
            (
                "mathematical_computation",
                IntentType::MathematicalComputation,
            ),
            ("ratio_trend_analysis", IntentType::RatioTrendAnalysis),
            (
                "contradiction_resolution",
                IntentType::ContradictionResolution,
            ),
            ("meta_memory", IntentType::MetaMemory),
            ("incremental_update", IntentType::IncrementalUpdate),
            ("causal_counterfactual", IntentType::CausalCounterfactual),
        ];
        for (s, expected) in cases {
            assert_eq!(IntentType::parse(s), expected);
            assert_eq!(IntentType::parse(s).to_string(), s);
        }
        assert_eq!(IntentType::parse("nonexistent"), IntentType::SimpleRecall);
    }

    #[test]
    fn intent_type_helpers() {
        assert!(IntentType::SimpleRecall.is_simple());
        assert!(IntentType::IncrementalUpdate.is_simple());
        assert!(!IntentType::MetaMemory.is_simple());
        assert!(IntentType::MetaMemory.is_aggregation());
        assert!(IntentType::MultiSourceSynthesis.is_complex());
    }

    #[test]
    fn config_defaults() {
        let cfg = SynthesisConfig::default();
        assert_eq!(cfg.max_facts_complex, 500);
        assert_eq!(cfg.max_facts_simple, 300);
        assert!(cfg.synthesis_temperature > 0.0 && cfg.simple_retrieval_threshold > 0);
    }

    #[test]
    fn detected_intent_serde() {
        let i = DetectedIntent {
            intent: IntentType::TemporalComparison,
            needs_temporal: true,
            ..Default::default()
        };
        let j = serde_json::to_string(&i).unwrap();
        let p: DetectedIntent = serde_json::from_str(&j).unwrap();
        assert_eq!(p.intent, IntentType::TemporalComparison);
        assert!(p.needs_temporal);
    }

    #[test]
    fn default_types() {
        let e = CompletenessEvaluation::default();
        assert!(!e.is_complete && e.gaps.is_empty());
        let t = TemporalCodeResult::default();
        assert!(t.code.is_empty() && t.result.is_none() && t.state_count == 0);
        #[allow(clippy::const_is_empty)]
        {
            assert!(!ENUMERATION_KEYWORDS.is_empty() && ENUMERATION_KEYWORDS.contains(&"list all"));
        }
    }
}
