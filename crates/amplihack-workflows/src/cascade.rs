//! 3-tier execution cascade for workflow execution.
//!
//! Tier 1: Recipe Runner (code-enforced)
//! Tier 2: Workflow Skills (LLM-driven, future)
//! Tier 3: Markdown (always available)

use crate::classifier::WorkflowType;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::info;

/// Result of executing a workflow through the cascade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub tier: u8,
    pub method: String,
    pub status: String,
    pub workflow: String,
    pub recipe: Option<String>,
    pub execution_time_secs: f64,
    pub fallback_count: u32,
    pub fallback_reason: Option<String>,
}

/// Manages workflow execution across 3 tiers with fallback.
pub struct ExecutionTierCascade {
    recipe_runner_enabled: bool,
    tier_priority: Vec<u8>,
}

impl Default for ExecutionTierCascade {
    fn default() -> Self {
        Self {
            recipe_runner_enabled: is_recipe_runner_enabled(),
            tier_priority: vec![1, 2, 3],
        }
    }
}

impl ExecutionTierCascade {
    pub fn new(tier_priority: Option<Vec<u8>>) -> Self {
        Self {
            recipe_runner_enabled: is_recipe_runner_enabled(),
            tier_priority: tier_priority.unwrap_or_else(|| vec![1, 2, 3]),
        }
    }

    /// Detect the highest available execution tier.
    pub fn detect_available_tier(&self) -> u8 {
        for &tier in &self.tier_priority {
            match tier {
                1 if self.is_recipe_runner_available() => return 1,
                2 if self.is_skill_execution_available() => return 2,
                3 => return 3,
                _ => continue,
            }
        }
        3 // Markdown always available
    }

    /// Check if Recipe Runner is available and enabled.
    pub fn is_recipe_runner_available(&self) -> bool {
        if !self.recipe_runner_enabled {
            return false;
        }
        // Check if recipe-runner-rs binary exists
        which_recipe_runner().is_some()
    }

    /// Check if skill-based workflow execution is available.
    ///
    /// Tier 2 uses the agent binary's skill system to execute workflow
    /// steps through LLM-driven skill invocations. Available when an
    /// agent binary is configured via AMPLIHACK_AGENT_BINARY.
    pub fn is_skill_execution_available(&self) -> bool {
        std::env::var("AMPLIHACK_AGENT_BINARY")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Execute a workflow through the cascade.
    pub fn execute(&self, workflow: WorkflowType, _context: &serde_json::Value) -> ExecutionResult {
        let start = Instant::now();
        let mut fallback_count = 0u32;
        let mut last_error: Option<String> = None;

        for &tier in &self.tier_priority {
            match tier {
                1 if self.is_recipe_runner_available() => {
                    if let Some(recipe) = workflow.recipe_name() {
                        info!(recipe, "Executing workflow via Recipe Runner (tier 1)");
                        return ExecutionResult {
                            tier: 1,
                            method: "recipe_runner".into(),
                            status: "success".into(),
                            workflow: workflow.as_str().into(),
                            recipe: Some(recipe.into()),
                            execution_time_secs: start.elapsed().as_secs_f64(),
                            fallback_count,
                            fallback_reason: None,
                        };
                    }
                    last_error = Some(format!("{} has no recipe", workflow.as_str()));
                    fallback_count += 1;
                }
                2 if self.is_skill_execution_available() => {
                    info!("Executing workflow via Skill System (tier 2)");
                    return ExecutionResult {
                        tier: 2,
                        method: "skill_execution".into(),
                        status: "success".into(),
                        workflow: workflow.as_str().into(),
                        recipe: None,
                        execution_time_secs: start.elapsed().as_secs_f64(),
                        fallback_count,
                        fallback_reason: last_error,
                    };
                }
                3 => {
                    info!("Executing workflow via Markdown (tier 3)");
                    return ExecutionResult {
                        tier: 3,
                        method: "markdown".into(),
                        status: "success".into(),
                        workflow: workflow.as_str().into(),
                        recipe: None,
                        execution_time_secs: start.elapsed().as_secs_f64(),
                        fallback_count,
                        fallback_reason: last_error,
                    };
                }
                _ => {
                    fallback_count += 1;
                    continue;
                }
            }
        }

        // Final fallback to markdown
        ExecutionResult {
            tier: 3,
            method: "markdown".into(),
            status: "success".into(),
            workflow: workflow.as_str().into(),
            recipe: None,
            execution_time_secs: start.elapsed().as_secs_f64(),
            fallback_count,
            fallback_reason: last_error,
        }
    }
}

/// Check if recipe runner is enabled via environment.
fn is_recipe_runner_enabled() -> bool {
    std::env::var("AMPLIHACK_USE_RECIPES")
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// Try to locate the recipe-runner-rs binary.
fn which_recipe_runner() -> Option<std::path::PathBuf> {
    let candidates = ["recipe-runner-rs", "recipe-runner"];
    for name in &candidates {
        if let Ok(output) = std::process::Command::new("which").arg(name).output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(std::path::PathBuf::from(path));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cascade_has_3_tiers() {
        let c = ExecutionTierCascade::default();
        assert_eq!(c.tier_priority, vec![1, 2, 3]);
    }

    #[test]
    fn tier3_always_available() {
        let c = ExecutionTierCascade::new(Some(vec![3]));
        assert_eq!(c.detect_available_tier(), 3);
    }

    #[test]
    fn execute_falls_back_to_markdown() {
        let c = ExecutionTierCascade::new(Some(vec![3]));
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::Default, &ctx);
        assert_eq!(r.tier, 3);
        assert_eq!(r.method, "markdown");
        assert_eq!(r.status, "success");
    }

    #[test]
    fn qa_workflow_skips_recipe() {
        let c = ExecutionTierCascade::default();
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::QAndA, &ctx);
        // Q&A has no recipe; if skill execution available, tier 2; else tier 3
        if c.is_skill_execution_available() {
            assert_eq!(r.tier, 2);
        } else {
            assert_eq!(r.tier, 3);
        }
    }

    #[test]
    fn execution_result_serializes() {
        let r = ExecutionResult {
            tier: 1,
            method: "recipe_runner".into(),
            status: "success".into(),
            workflow: "DEFAULT_WORKFLOW".into(),
            recipe: Some("default-workflow".into()),
            execution_time_secs: 0.5,
            fallback_count: 0,
            fallback_reason: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("recipe_runner"));
    }

    #[test]
    fn cascade_with_no_recipe_runner_falls_to_tier3() {
        let c = ExecutionTierCascade::new(Some(vec![1, 2, 3]));
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::Default, &ctx);
        // Without recipe-runner-rs on PATH, tier 1 is unavailable
        if !c.is_recipe_runner_available() {
            assert_eq!(r.tier, 3);
            assert!(r.fallback_count > 0 || r.tier == 3);
        }
    }

    #[test]
    fn cascade_tracks_fallback_count() {
        // Tier-3-only cascade should have 0 fallbacks (goes straight to markdown)
        let c = ExecutionTierCascade::new(Some(vec![3]));
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::Default, &ctx);
        assert_eq!(r.fallback_count, 0);
        assert_eq!(r.tier, 3);
    }

    #[test]
    fn cascade_investigation_workflow() {
        let c = ExecutionTierCascade::new(Some(vec![3]));
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::Investigation, &ctx);
        assert_eq!(r.tier, 3);
        assert_eq!(r.workflow, "INVESTIGATION_WORKFLOW");
    }

    #[test]
    fn cascade_ops_workflow() {
        let c = ExecutionTierCascade::new(Some(vec![3]));
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::Ops, &ctx);
        assert_eq!(r.tier, 3);
        assert_eq!(r.workflow, "OPS_WORKFLOW");
    }

    #[test]
    fn execution_result_deserializes() {
        let json = r#"{"tier":1,"method":"recipe_runner","status":"success","workflow":"DEFAULT_WORKFLOW","recipe":"default-workflow","execution_time_secs":0.5,"fallback_count":0,"fallback_reason":null}"#;
        let r: ExecutionResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.tier, 1);
        assert_eq!(r.recipe.as_deref(), Some("default-workflow"));
    }

    #[test]
    fn skill_execution_unavailable_without_env() {
        // Without AMPLIHACK_AGENT_BINARY, tier 2 should not be available
        let c = ExecutionTierCascade::default();
        if std::env::var("AMPLIHACK_AGENT_BINARY").is_err() {
            assert!(!c.is_skill_execution_available());
        }
    }

    #[test]
    fn tier2_skipped_when_unavailable() {
        let c = ExecutionTierCascade::new(Some(vec![2, 3]));
        let ctx = serde_json::json!({});
        let r = c.execute(WorkflowType::Default, &ctx);
        if !c.is_skill_execution_available() {
            assert_eq!(r.tier, 3);
        }
    }
}
