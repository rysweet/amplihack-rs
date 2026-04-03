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
        } else if lower.contains("log ")
            || lower.contains(" log")
            || lower.contains("logs")
            || lower.contains("logging")
            || lower.contains("log-analysis")
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzer() -> PromptAnalyzer {
        PromptAnalyzer::new()
    }

    #[test]
    fn analyze_rejects_empty_prompt() {
        assert!(analyzer().analyze("").is_err());
        assert!(analyzer().analyze("   ").is_err());
    }

    #[test]
    fn analyze_returns_structured_goal() {
        let g = analyzer().analyze("scan for vulnerabilities").unwrap();
        assert_eq!(g.raw_prompt, "scan for vulnerabilities");
        assert_eq!(g.domain, "security");
    }

    // Domain detection ---------------------------------------------------

    #[test]
    fn detect_domain_security() {
        let g = analyzer().analyze("scan for CVE issues").unwrap();
        assert_eq!(g.domain, "security");
    }

    #[test]
    fn detect_domain_data_processing() {
        let g = analyzer().analyze("aggregate csv data").unwrap();
        assert_eq!(g.domain, "data-processing");
    }

    #[test]
    fn detect_domain_log_analysis() {
        let g = analyzer().analyze("parse logs for errors").unwrap();
        assert_eq!(g.domain, "log-analysis");
    }

    #[test]
    fn detect_domain_code_review() {
        let g = analyzer().analyze("review pull request").unwrap();
        assert_eq!(g.domain, "code-review");
    }

    #[test]
    fn detect_domain_meetings() {
        let g = analyzer().analyze("summarize meeting notes").unwrap();
        assert_eq!(g.domain, "meetings");
    }

    #[test]
    fn detect_domain_email() {
        let g = analyzer().analyze("draft email response").unwrap();
        assert_eq!(g.domain, "email");
    }

    #[test]
    fn detect_domain_development() {
        let g = analyzer().analyze("build the project").unwrap();
        assert_eq!(g.domain, "development");
    }

    #[test]
    fn detect_domain_file_management() {
        let g = analyzer().analyze("organize the directory").unwrap();
        assert_eq!(g.domain, "file-management");
    }

    #[test]
    fn detect_domain_general_fallback() {
        let g = analyzer().analyze("hello world").unwrap();
        assert_eq!(g.domain, "general");
    }

    // Complexity estimation ----------------------------------------------

    #[test]
    fn complexity_simple_for_short_prompt() {
        let g = analyzer().analyze("fix bug").unwrap();
        assert_eq!(g.complexity, Complexity::Simple);
    }

    #[test]
    fn complexity_moderate_for_medium_prompt() {
        let prompt = "a".repeat(60);
        let g = analyzer().analyze(&prompt).unwrap();
        assert_eq!(g.complexity, Complexity::Moderate);
    }

    #[test]
    fn complexity_complex_for_long_prompt() {
        let prompt = "a".repeat(120);
        let g = analyzer().analyze(&prompt).unwrap();
        assert_eq!(g.complexity, Complexity::Complex);
    }

    // Constraint extraction ----------------------------------------------

    #[test]
    fn extracts_constraints() {
        let g = analyzer()
            .analyze("build app. It must be fast. Should handle errors.")
            .unwrap();
        assert!(g.constraints.iter().any(|c| c.contains("must")));
        assert!(g.constraints.iter().any(|c| c.contains("Should")));
    }

    // Success criteria extraction ----------------------------------------

    #[test]
    fn extracts_success_criteria() {
        let g = analyzer()
            .analyze("deploy service. Done when all tests pass. Success means zero errors.")
            .unwrap();
        assert!(!g.success_criteria.is_empty());
    }

    #[test]
    fn default_impl() {
        let a = PromptAnalyzer::default();
        assert!(a.analyze("test").is_ok());
    }
}
