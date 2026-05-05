//! Known amplihack skill names.
//!
//! Provides a compile-time set of all built-in skill names and a fast
//! membership check via `is_amplihack_skill()`.

use std::collections::HashSet;
use std::sync::LazyLock;

/// All built-in amplihack skill names.
static AMPLIHACK_SKILLS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "agent-generator-tutor",
        "amplihack-expert",
        "anthropologist-analyst",
        "aspire",
        "authenticated-web-scraper",
        "awesome-copilot-sync",
        "azure-admin",
        "azure-devops",
        "azure-devops-cli",
        "backlog-curator",
        "biologist-analyst",
        "cascade-workflow",
        "chemist-analyst",
        "claude-agent-sdk",
        "code-smell-detector",
        "code-visualizer",
        "computer-scientist-analyst",
        "consensus-voting",
        "context-management",
        "crusty-old-engineer",
        "cybersecurity-analyst",
        "debate-workflow",
        "default-workflow",
        "dependency-resolver",
        "design-patterns-expert",
        "dev-orchestrator",
        "development/architecting-solutions",
        "development/setting-up-projects",
        "documentation-writing",
        "docx",
        "dotnet-exception-handling",
        "dotnet-install",
        "dotnet10-pack-tool",
        "dynamic-debugger",
        "e2e-outside-in-test-generator",
        "economist-analyst",
        "email-drafter",
        "engineer-analyst",
        "environmentalist-analyst",
        "epidemiologist-analyst",
        "ethicist-analyst",
        "eval-recipes-runner",
        "futurist-analyst",
        "gh-work-report",
        "gherkin-expert",
        "github-copilot-cli-expert",
        "goal-seeking-agent-pattern",
        "historian-analyst",
        "indigenous-leader-analyst",
        "investigation-workflow",
        "journalist-analyst",
        "knowledge-extractor",
        "lawyer-analyst",
        "learning-path-builder",
        "mcp-manager",
        "meeting-synthesizer",
        "mermaid-diagram-generator",
        "meta-cognitive/analyzing-deeply",
        "microsoft-agent-framework",
        "model-evaluation-benchmark",
        "module-spec-generator",
        "multi-repo",
        "n-version-workflow",
        "novelist-analyst",
        "outside-in-testing",
        "pdf",
        "philosopher-analyst",
        "philosophy-compliance-workflow",
        "physicist-analyst",
        "pm-architect",
        "poet-analyst",
        "political-scientist-analyst",
        "pptx",
        "pr-review-assistant",
        "psychologist-analyst",
        "qa-team",
        "quality-audit",
        "quality/reviewing-code",
        "quality/testing-code",
        "remote-work",
        "research/researching-topics",
        "roadmap-strategist",
        "session-learning",
        "session-replay",
        "session-to-agent",
        "skill-builder",
        "smart-test",
        "sociologist-analyst",
        "storytelling-synthesizer",
        "supply-chain-audit",
        "test-gap-analyzer",
        "transcript-viewer",
        "ultrathink-orchestrator",
        "urban-planner-analyst",
        "work-delegator",
        "workflow-enforcement",
        "workstream-coordinator",
        "xlsx",
    ])
});

/// Check whether `name` is a known amplihack skill.
pub fn is_amplihack_skill(name: &str) -> bool {
    AMPLIHACK_SKILLS.contains(name)
}

/// Return the total number of known skills.
pub fn skill_count() -> usize {
    AMPLIHACK_SKILLS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_skill_is_recognised() {
        assert!(is_amplihack_skill("default-workflow"));
        assert!(is_amplihack_skill("pdf"));
        assert!(is_amplihack_skill("xlsx"));
    }

    #[test]
    fn unknown_skill_is_rejected() {
        assert!(!is_amplihack_skill("nonexistent-skill"));
        assert!(!is_amplihack_skill(""));
        assert!(!is_amplihack_skill("DEFAULT-WORKFLOW")); // case-sensitive
    }

    #[test]
    fn path_style_skills_work() {
        assert!(is_amplihack_skill("development/architecting-solutions"));
        assert!(is_amplihack_skill("quality/reviewing-code"));
        assert!(is_amplihack_skill("meta-cognitive/analyzing-deeply"));
    }

    #[test]
    fn skill_count_is_reasonable() {
        let count = skill_count();
        assert!(
            (85..=120).contains(&count),
            "expected ~90 skills, got {count}"
        );
    }

    #[test]
    fn all_skills_are_lowercase_or_slashed() {
        for skill in AMPLIHACK_SKILLS.iter() {
            assert!(
                skill
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c == '/' || c.is_ascii_digit()),
                "unexpected characters in skill name: {skill}"
            );
        }
    }
}
