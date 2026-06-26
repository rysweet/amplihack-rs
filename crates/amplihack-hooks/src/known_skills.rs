//! Known amplihack skill names.
//!
//! Provides a compile-time set of all built-in skill names and a fast
//! membership check via `is_amplihack_skill()`.

/// All built-in amplihack skill names. Keep sorted for `binary_search`.
static AMPLIHACK_SKILLS: &[&str] = &[
    "agent-generator-tutor",
    "amplihack-expert",
    "amplihack-migrate",
    "analyzing-deeply",
    "anthropologist-analyst",
    "architecting-solutions",
    "aspire",
    "authenticated-web-scraper",
    "awesome-copilot-sync",
    "azure-admin",
    "azure-devops",
    "backlog-curator",
    "biologist-analyst",
    "cascade-workflow",
    "chemist-analyst",
    "claude-agent-sdk",
    "code-atlas",
    "code-philosophy",
    "code-smell-detector",
    "code-visualizer",
    "computer-scientist-analyst",
    "consensus-voting",
    "context-management",
    "creating-pull-requests",
    "crusty-old-engineer",
    "cybersecurity-analyst",
    "debate-workflow",
    "default-workflow",
    "dependency-resolver",
    "design-patterns-expert",
    "dev-orchestrator",
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
    "fleet",
    "fleet-copilot",
    "futurist-analyst",
    "gh-aw-adoption",
    "gh-work-report",
    "gherkin-expert",
    "github",
    "github-copilot-cli",
    "github-copilot-cli-expert",
    "github-copilot-sdk",
    "goal-seeking-agent-pattern",
    "historian-analyst",
    "indigenous-leader-analyst",
    "investigation-workflow",
    "journalist-analyst",
    "knowledge-extractor",
    "lawyer-analyst",
    "learning-path-builder",
    "lsp-setup",
    "markitdown",
    "mcp-manager",
    "meeting-synthesizer",
    "merge-ready",
    "mermaid-diagram-generator",
    "microsoft-agent-framework",
    "model-evaluation-benchmark",
    "module-spec-generator",
    "multi-repo",
    "multitask",
    "n-version-workflow",
    "novelist-analyst",
    "outside-in-testing",
    "oxidizer-workflow",
    "pdf",
    "philosopher-analyst",
    "philosophy-compliance-workflow",
    "physicist-analyst",
    "pm-architect",
    "poet-analyst",
    "political-scientist-analyst",
    "pptx",
    "pr-guide",
    "pr-review-assistant",
    "pre-commit-manager",
    "psychologist-analyst",
    "qa-team",
    "quality-audit",
    "remote-work",
    "researching-topics",
    "reviewing-code",
    "roadmap-strategist",
    "self-improving-agent-builder",
    "session-learning",
    "session-replay",
    "session-to-agent",
    "setting-up-projects",
    "shadow-testing",
    "silent-degradation-audit",
    "skill-builder",
    "smart-test",
    "sociologist-analyst",
    "socratic-review",
    "statler-waldorf",
    "storytelling-synthesizer",
    "supply-chain-audit",
    "test-gap-analyzer",
    "testing-code",
    "tla-plus-expert",
    "transcript-viewer",
    "ultrathink-orchestrator",
    "urban-planner-analyst",
    "work-delegator",
    "work-iq",
    "workflow-enforcement",
    "workiq-wsl",
    "workstream-coordinator",
    "xlsx",
];

/// Check whether `name` is a known amplihack skill.
pub fn is_amplihack_skill(name: &str) -> bool {
    AMPLIHACK_SKILLS.binary_search(&name).is_ok()
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
    fn canonical_skill_names_work() {
        assert!(is_amplihack_skill("architecting-solutions"));
        assert!(is_amplihack_skill("reviewing-code"));
        assert!(is_amplihack_skill("analyzing-deeply"));
        assert!(!is_amplihack_skill("development/architecting-solutions"));
        assert!(!is_amplihack_skill("quality/reviewing-code"));
        assert!(!is_amplihack_skill("meta-cognitive/analyzing-deeply"));
    }

    #[test]
    fn skill_count_is_reasonable() {
        assert_eq!(skill_count(), 122);
    }

    #[test]
    fn all_skills_are_kebab_case() {
        for skill in AMPLIHACK_SKILLS.iter() {
            assert!(
                skill
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "unexpected characters in skill name: {skill}"
            );
        }
    }

    #[test]
    fn skills_are_sorted_for_binary_search() {
        for pair in AMPLIHACK_SKILLS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "skill list must stay sorted: {} before {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn registry_matches_bundled_skill_frontmatter_names() {
        use std::collections::BTreeSet;
        use std::fs;
        use std::path::{Path, PathBuf};

        fn collect_skill_files(dir: &Path, files: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read bundled skills dir") {
                let path = entry.expect("read bundled skills entry").path();
                if path.is_dir() {
                    collect_skill_files(&path, files);
                } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                    files.push(path);
                }
            }
        }

        fn frontmatter_name(path: &Path) -> String {
            let content = fs::read_to_string(path).expect("read bundled SKILL.md");
            let frontmatter = content
                .strip_prefix("---\n")
                .and_then(|rest| rest.split_once("\n---"))
                .map(|(frontmatter, _)| frontmatter)
                .unwrap_or_else(|| panic!("SKILL.md frontmatter missing: {}", path.display()));
            frontmatter
                .lines()
                .find_map(|line| line.trim().strip_prefix("name:"))
                .map(|name| name.trim().to_string())
                .unwrap_or_else(|| panic!("SKILL.md name missing: {}", path.display()))
        }

        let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("amplifier-bundle/skills");
        let mut skill_files = Vec::new();
        collect_skill_files(&skills_dir, &mut skill_files);

        let bundled: BTreeSet<_> = skill_files
            .iter()
            .map(|path| frontmatter_name(path))
            .collect();
        let registered: BTreeSet<_> = AMPLIHACK_SKILLS
            .iter()
            .map(|skill| (*skill).to_string())
            .collect();

        assert_eq!(
            bundled, registered,
            "known skill registry must match bundled SKILL.md frontmatter names"
        );
        assert!(
            !registered.contains("azure-devops-cli"),
            "azure-devops-cli is supporting documentation, not a loadable skill"
        );
    }
}
