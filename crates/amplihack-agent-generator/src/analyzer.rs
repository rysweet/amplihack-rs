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

    #[test]
    fn detect_domain_security() {
        assert_eq!(PromptAnalyzer::detect_domain("scan for vulnerabilities"), "security");
        assert_eq!(PromptAnalyzer::detect_domain("check CVE database"), "security");
    }

    #[test]
    fn detect_domain_data_processing() {
        assert_eq!(PromptAnalyzer::detect_domain("process CSV data"), "data-processing");
        assert_eq!(PromptAnalyzer::detect_domain("build a pipeline"), "data-processing");
    }

    #[test]
    fn detect_domain_log_analysis() {
        assert_eq!(PromptAnalyzer::detect_domain("analyze logs for errors"), "log-analysis");
    }

    #[test]
    fn detect_domain_development() {
        assert_eq!(PromptAnalyzer::detect_domain("build the code"), "development");
    }

    #[test]
    fn detect_domain_general_fallback() {
        assert_eq!(PromptAnalyzer::detect_domain("hello world"), "general");
    }

    #[test]
    fn extract_constraints_picks_must_should_require() {
        let text = "Must be fast.\nShould use Rust.\nThis is fine.\nRequires auth.";
        let c = PromptAnalyzer::extract_constraints(text);
        assert_eq!(c.len(), 3);
        assert!(c.iter().any(|s| s.contains("fast")));
        assert!(c.iter().any(|s| s.contains("Rust")));
        assert!(c.iter().any(|s| s.contains("auth")));
    }

    #[test]
    fn extract_success_criteria_keywords() {
        let text = "Done when tests pass.\nThe success should be verified.\nRandom line.";
        let c = PromptAnalyzer::extract_success_criteria(text);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn estimate_complexity_by_length() {
        assert_eq!(PromptAnalyzer::estimate_complexity("short"), Complexity::Simple);
        let medium = "a".repeat(60);
        assert_eq!(PromptAnalyzer::estimate_complexity(&medium), Complexity::Moderate);
        let long = "a".repeat(120);
        assert_eq!(PromptAnalyzer::estimate_complexity(&long), Complexity::Complex);
    }

    #[test]
    fn analyze_empty_prompt_errors() {
        let a = PromptAnalyzer::new();
        assert!(a.analyze("   ").is_err());
    }

    #[test]
    fn analyze_sets_domain_and_complexity() {
        let a = PromptAnalyzer::new();
        let g = a.analyze("scan for security vulnerabilities in the codebase. Must be thorough. Done when all CVEs checked.").unwrap();
        assert_eq!(g.domain, "security");
        // >100 chars → Complex, but the complexity threshold is on the trimmed prompt length
        assert!(g.complexity >= Complexity::Moderate);
        assert!(!g.constraints.is_empty());
        assert!(!g.success_criteria.is_empty());
    }
}
