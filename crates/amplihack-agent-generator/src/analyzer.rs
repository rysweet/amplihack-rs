use crate::error::{GeneratorError, Result};
use crate::models::{Complexity, GoalDefinition};

/// Analyzes a raw user prompt to extract a structured [`GoalDefinition`].
pub struct PromptAnalyzer;

impl PromptAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Parse *prompt* into a structured goal with domain, constraints, and
    /// complexity classification.
    pub fn analyze(&self, prompt: &str) -> Result<GoalDefinition> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Err(GeneratorError::InvalidGoal(
                "prompt must not be empty".into(),
            ));
        }

        let domain = Self::detect_domain(trimmed);
        let constraints = Self::extract_constraints(trimmed);
        let success_criteria = Self::extract_success_criteria(trimmed);
        let complexity = Self::estimate_complexity(trimmed);

        let mut goal = GoalDefinition::new(prompt, trimmed, &domain)?;
        goal.constraints = constraints;
        goal.success_criteria = success_criteria;
        goal.complexity = complexity;

        Ok(goal)
    }

    fn detect_domain(prompt: &str) -> String {
        let lower = prompt.to_lowercase();

        if lower.contains("security")
            || lower.contains("scan")
            || lower.contains("vulnerabilit")
            || lower.contains("cve")
        {
            "security".into()
        } else if lower.contains("data")
            || lower.contains("csv")
            || lower.contains("pipeline")
            || lower.contains("aggregate")
        {
            "data-processing".into()
        } else if lower.contains("log") {
            "log-analysis".into()
        } else if lower.contains("review") {
            "code-review".into()
        } else if lower.contains("meeting") {
            "meetings".into()
        } else if lower.contains("email") {
            "email".into()
        } else if lower.contains("code")
            || lower.contains("build")
            || lower.contains("develop")
            || lower.contains("compile")
        {
            "development".into()
        } else if lower.contains("file") || lower.contains("directory") {
            "file-management".into()
        } else {
            "general".into()
        }
    }

    fn extract_constraints(prompt: &str) -> Vec<String> {
        let mut constraints = Vec::new();
        for line in prompt.split('\n') {
            for sentence in line.split('.') {
                let trimmed = sentence.trim().trim_start_matches('-').trim();
                if trimmed.is_empty() {
                    continue;
                }
                let lower = trimmed.to_lowercase();
                if lower.contains("must") || lower.contains("should") || lower.contains("require") {
                    constraints.push(trimmed.to_string());
                }
            }
        }
        constraints
    }

    fn extract_success_criteria(prompt: &str) -> Vec<String> {
        let mut criteria = Vec::new();
        for line in prompt.split('\n') {
            for sentence in line.split('.') {
                let trimmed = sentence.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let lower = trimmed.to_lowercase();
                if lower.contains("success")
                    || lower.contains("complete")
                    || lower.contains("done when")
                {
                    criteria.push(trimmed.to_string());
                }
            }
        }
        criteria
    }

    fn estimate_complexity(prompt: &str) -> Complexity {
        let len = prompt.len();
        if len < 50 {
            Complexity::Simple
        } else if len < 100 {
            Complexity::Moderate
        } else {
            Complexity::Complex
        }
    }
}

impl Default for PromptAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
