//! Test level definitions for the progressive eval framework (L1-L12).

use serde::{Deserialize, Serialize};
use std::fmt;

/// The 12 progressive evaluation levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestLevel {
    L1Recall,
    L2MultiSourceSynthesis,
    L3TemporalReasoning,
    L4ProceduralLearning,
    L5ContradictionHandling,
    L6IncrementalLearning,
    L7TeacherStudent,
    L8Metacognition,
    L9CausalReasoning,
    L10CounterfactualReasoning,
    L11NovelSkillAcquisition,
    L12FarTransfer,
}

impl TestLevel {
    /// All levels in order.
    pub fn all() -> &'static [TestLevel] {
        &[
            TestLevel::L1Recall,
            TestLevel::L2MultiSourceSynthesis,
            TestLevel::L3TemporalReasoning,
            TestLevel::L4ProceduralLearning,
            TestLevel::L5ContradictionHandling,
            TestLevel::L6IncrementalLearning,
            TestLevel::L7TeacherStudent,
            TestLevel::L8Metacognition,
            TestLevel::L9CausalReasoning,
            TestLevel::L10CounterfactualReasoning,
            TestLevel::L11NovelSkillAcquisition,
            TestLevel::L12FarTransfer,
        ]
    }

    /// Numeric level identifier (1-12).
    pub fn id(&self) -> u8 {
        match self {
            TestLevel::L1Recall => 1,
            TestLevel::L2MultiSourceSynthesis => 2,
            TestLevel::L3TemporalReasoning => 3,
            TestLevel::L4ProceduralLearning => 4,
            TestLevel::L5ContradictionHandling => 5,
            TestLevel::L6IncrementalLearning => 6,
            TestLevel::L7TeacherStudent => 7,
            TestLevel::L8Metacognition => 8,
            TestLevel::L9CausalReasoning => 9,
            TestLevel::L10CounterfactualReasoning => 10,
            TestLevel::L11NovelSkillAcquisition => 11,
            TestLevel::L12FarTransfer => 12,
        }
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            TestLevel::L1Recall => "Recall",
            TestLevel::L2MultiSourceSynthesis => "Multi-Source Synthesis",
            TestLevel::L3TemporalReasoning => "Temporal Reasoning",
            TestLevel::L4ProceduralLearning => "Procedural Learning",
            TestLevel::L5ContradictionHandling => "Contradiction Handling",
            TestLevel::L6IncrementalLearning => "Incremental Learning",
            TestLevel::L7TeacherStudent => "Teacher-Student",
            TestLevel::L8Metacognition => "Metacognition",
            TestLevel::L9CausalReasoning => "Causal Reasoning",
            TestLevel::L10CounterfactualReasoning => "Counterfactual Reasoning",
            TestLevel::L11NovelSkillAcquisition => "Novel Skill Acquisition",
            TestLevel::L12FarTransfer => "Far Transfer",
        }
    }

    /// Description of what this level tests.
    pub fn description(&self) -> &'static str {
        match self {
            TestLevel::L1Recall => "Direct retrieval of stored facts",
            TestLevel::L2MultiSourceSynthesis => {
                "Combining information from multiple memory sources"
            }
            TestLevel::L3TemporalReasoning => "Reasoning about time-ordered events",
            TestLevel::L4ProceduralLearning => "Learning and executing multi-step procedures",
            TestLevel::L5ContradictionHandling => {
                "Detecting and resolving contradictory information"
            }
            TestLevel::L6IncrementalLearning => "Building on previously learned knowledge",
            TestLevel::L7TeacherStudent => "Teaching concepts to another agent",
            TestLevel::L8Metacognition => "Reasoning about own knowledge and limitations",
            TestLevel::L9CausalReasoning => "Understanding cause-and-effect relationships",
            TestLevel::L10CounterfactualReasoning => "Reasoning about hypothetical alternatives",
            TestLevel::L11NovelSkillAcquisition => "Learning entirely new capabilities",
            TestLevel::L12FarTransfer => "Applying knowledge to very different domains",
        }
    }

    /// Difficulty score (1.0 = easiest, 12.0 = hardest).
    pub fn difficulty(&self) -> f64 {
        self.id() as f64
    }

    /// Minimum score required to pass this level.
    pub fn passing_threshold(&self) -> f64 {
        match self {
            TestLevel::L1Recall => 0.9,
            TestLevel::L2MultiSourceSynthesis => 0.85,
            TestLevel::L3TemporalReasoning => 0.8,
            TestLevel::L4ProceduralLearning => 0.75,
            TestLevel::L5ContradictionHandling => 0.7,
            TestLevel::L6IncrementalLearning => 0.7,
            TestLevel::L7TeacherStudent => 0.65,
            TestLevel::L8Metacognition => 0.6,
            TestLevel::L9CausalReasoning => 0.6,
            TestLevel::L10CounterfactualReasoning => 0.55,
            TestLevel::L11NovelSkillAcquisition => 0.5,
            TestLevel::L12FarTransfer => 0.5,
        }
    }

    /// Parse from a string like "L1", "l3", "L12", or a display name.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let s_lower = s.to_lowercase();
        // Try "lN" format
        if let Some(stripped) = s_lower.strip_prefix('l')
            && let Ok(n) = stripped.parse::<u8>()
        {
            return Self::from_id(n);
        }
        // Try display name match
        Self::all()
            .iter()
            .find(|level| level.display_name().to_lowercase() == s_lower)
            .copied()
    }

    /// Look up a level by its numeric id.
    pub fn from_id(id: u8) -> Option<Self> {
        Self::all().iter().find(|level| level.id() == id).copied()
    }
}

impl fmt::Display for TestLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{} {}", self.id(), self.display_name())
    }
}
