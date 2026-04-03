use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{GeneratorError, Result};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
    Simple,
    Moderate,
    Complex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum BundleStatus {
    #[default]
    Pending,
    Planning,
    Assembling,
    Ready,
    Failed,
}

// ---------------------------------------------------------------------------
// GoalDefinition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalDefinition {
    pub raw_prompt: String,
    pub goal: String,
    pub domain: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
    #[serde(default = "default_complexity")]
    pub complexity: Complexity,
}

fn default_complexity() -> Complexity {
    Complexity::Simple
}

impl GoalDefinition {
    pub fn new(
        raw_prompt: impl Into<String>,
        goal: impl Into<String>,
        domain: impl Into<String>,
    ) -> Result<Self> {
        let raw_prompt = raw_prompt.into();
        let goal = goal.into();
        let domain = domain.into();

        if raw_prompt.trim().is_empty() {
            return Err(GeneratorError::InvalidGoal(
                "raw_prompt must not be empty".into(),
            ));
        }
        if goal.trim().is_empty() {
            return Err(GeneratorError::InvalidGoal("goal must not be empty".into()));
        }
        if domain.trim().is_empty() {
            return Err(GeneratorError::InvalidGoal(
                "domain must not be empty".into(),
            ));
        }

        Ok(Self {
            raw_prompt,
            goal,
            domain,
            constraints: Vec::new(),
            success_criteria: Vec::new(),
            context: HashMap::new(),
            complexity: Complexity::Simple,
        })
    }
}

// ---------------------------------------------------------------------------
// PlanPhase
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPhase {
    pub name: String,
    pub description: String,
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub estimated_duration: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default = "default_true")]
    pub parallel_safe: bool,
    #[serde(default)]
    pub success_indicators: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl PlanPhase {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        required_capabilities: Vec<String>,
    ) -> Result<Self> {
        let name = name.into();
        let description = description.into();

        if name.trim().is_empty() {
            return Err(GeneratorError::PlanningFailed(
                "phase name must not be empty".into(),
            ));
        }
        if required_capabilities.is_empty() {
            return Err(GeneratorError::PlanningFailed(
                "required_capabilities must not be empty".into(),
            ));
        }

        Ok(Self {
            name,
            description,
            required_capabilities,
            estimated_duration: String::new(),
            dependencies: Vec::new(),
            parallel_safe: true,
            success_indicators: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// ExecutionPlan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub goal_id: Uuid,
    pub phases: Vec<PlanPhase>,
    #[serde(default)]
    pub total_estimated_duration: String,
    #[serde(default)]
    pub required_skills: Vec<String>,
    #[serde(default)]
    pub parallel_opportunities: Vec<Vec<String>>,
    #[serde(default)]
    pub risk_factors: Vec<String>,
}

impl ExecutionPlan {
    pub fn new(goal_id: Uuid, phases: Vec<PlanPhase>) -> Result<Self> {
        if phases.is_empty() {
            return Err(GeneratorError::PlanningFailed(
                "execution plan must have at least 1 phase".into(),
            ));
        }
        if phases.len() > 10 {
            return Err(GeneratorError::PlanningFailed(format!(
                "execution plan must have at most 10 phases, got {}",
                phases.len()
            )));
        }

        Ok(Self {
            goal_id,
            phases,
            total_estimated_duration: String::new(),
            required_skills: Vec::new(),
            parallel_opportunities: Vec::new(),
            risk_factors: Vec::new(),
        })
    }

    pub fn phase_count(&self) -> usize {
        self.phases.len()
    }
}

// ---------------------------------------------------------------------------
// SkillDefinition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub source_path: PathBuf,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub content: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub match_score: f64,
}

impl SkillDefinition {
    pub fn new(
        name: impl Into<String>,
        source_path: PathBuf,
        content: impl Into<String>,
    ) -> Result<Self> {
        let name = name.into();
        let content = content.into();

        if name.trim().is_empty() {
            return Err(GeneratorError::SynthesisFailed(
                "skill name must not be empty".into(),
            ));
        }
        if content.trim().is_empty() {
            return Err(GeneratorError::SynthesisFailed(
                "skill content must not be empty".into(),
            ));
        }

        Ok(Self {
            name,
            source_path,
            capabilities: Vec::new(),
            description: String::new(),
            content,
            dependencies: Vec::new(),
            match_score: 0.0,
        })
    }

    pub fn with_match_score(mut self, score: f64) -> Result<Self> {
        Self::validate_match_score(score)?;
        self.match_score = score;
        Ok(self)
    }

    pub fn validate_match_score(score: f64) -> Result<()> {
        if !(0.0..=1.0).contains(&score) {
            return Err(GeneratorError::SynthesisFailed(format!(
                "match_score must be between 0.0 and 1.0, got {score}"
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SDKToolConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SDKToolConfig {
    pub name: String,
    pub description: String,
    pub category: String,
}

impl SDKToolConfig {
    pub fn to_dict(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("name".into(), self.name.clone());
        map.insert("description".into(), self.description.clone());
        map.insert("category".into(), self.category.clone());
        map
    }
}

// ---------------------------------------------------------------------------
// SubAgentConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    pub role: String,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub filename: String,
}

impl SubAgentConfig {
    pub fn new(role: impl Into<String>) -> Self {
        let role = role.into();
        let filename = format!("{}.yaml", role);
        Self {
            role,
            config: HashMap::new(),
            filename,
        }
    }
}

// ---------------------------------------------------------------------------
// GoalAgentBundle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalAgentBundle {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_definition: Option<GoalDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_plan: Option<ExecutionPlan>,
    #[serde(default)]
    pub skills: Vec<SkillDefinition>,
    #[serde(default)]
    pub sdk_tools: Vec<SDKToolConfig>,
    #[serde(default)]
    pub sub_agent_configs: Vec<SubAgentConfig>,
    #[serde(default)]
    pub status: BundleStatus,
}

impl GoalAgentBundle {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.len() < 3 {
            return Err(GeneratorError::AssemblyFailed(format!(
                "bundle name must be at least 3 characters, got {}",
                name.len()
            )));
        }
        if name.len() > 50 {
            return Err(GeneratorError::AssemblyFailed(format!(
                "bundle name must be at most 50 characters, got {}",
                name.len()
            )));
        }

        Ok(Self {
            id: Uuid::new_v4(),
            name,
            version: version.into(),
            goal_definition: None,
            execution_plan: None,
            skills: Vec::new(),
            sdk_tools: Vec::new(),
            sub_agent_configs: Vec::new(),
            status: BundleStatus::Pending,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.goal_definition.is_some()
            && self.execution_plan.is_some()
            && !self.skills.is_empty()
            && self.status == BundleStatus::Ready
    }
}

// ---------------------------------------------------------------------------
// GenerationMetrics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetrics {
    pub total_time_seconds: f64,
    pub analysis_time: f64,
    pub planning_time: f64,
    pub synthesis_time: f64,
    pub assembly_time: f64,
    pub skill_count: usize,
    pub phase_count: usize,
    pub bundle_size_kb: f64,
}

impl GenerationMetrics {
    pub fn average_phase_time(&self) -> f64 {
        if self.phase_count == 0 {
            return 0.0;
        }
        self.total_time_seconds / self.phase_count as f64
    }
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
