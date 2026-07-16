//! Known amplihack agent names.
//!
//! Provides a compile-time set of all built-in agent names and a fast
//! membership check via `is_amplihack_agent()`.
//!
//! This registry tracks amplihack *agent* names — the
//! `amplifier-bundle/agents/**/*.md` definitions that are invoked via the
//! agent/Task runtime interface rather than the Skill tool. It exists so the
//! pre-tool-use hook can detect when a `Skill` invocation names something that
//! is actually an agent (e.g. `prompt-writer`) and redirect the model to the
//! correct interface instead of letting the copilot runtime hard-fail with
//! "Skill not found" (issue #838).

/// All built-in amplihack agent names. Keep sorted for `binary_search`.
///
/// Derived from the unique `.md` basenames under `amplifier-bundle/agents/`
/// (the `guide` basename appears under both `agents/` and `agents/core/` and is
/// deduped to a single entry). The `registry_matches_bundled_agent_files` test
/// fails the build if this list drifts from the bundled agent definitions.
static AMPLIHACK_AGENTS: &[&str] = &[
    "ambiguity",
    "amplifier-cli-architect",
    "amplihack-improvement-workflow",
    "analyzer",
    "api-designer",
    "architect",
    "azure-kubernetes-expert",
    "builder",
    "ci-diagnostic-workflow",
    "cleanup",
    "concept-extractor",
    "database",
    "documentation-writer",
    "fallback-cascade",
    "fix-agent",
    "gherkin-expert",
    "guide",
    "iac-planner",
    "insight-synthesizer",
    "integration",
    "knowledge-archaeologist",
    "mcp-server-builder",
    "multi-agent-debate",
    "n-version-validator",
    "openapi-scaffolder",
    "optimizer",
    "patterns",
    "philosophy-guardian",
    "pre-commit-diagnostic",
    "preference-reviewer",
    "prompt-review-workflow",
    "prompt-writer",
    "reviewer",
    "rust-programming-expert",
    "security",
    "socratic-reviewer",
    "tester",
    "tla-plus-expert",
    "visualization-architect",
    "worktree-manager",
    "xpia-defense",
];

/// Check whether `name` is a known amplihack agent.
pub fn is_amplihack_agent(name: &str) -> bool {
    AMPLIHACK_AGENTS.binary_search(&name).is_ok()
}

/// Return the total number of known agents.
pub fn agent_count() -> usize {
    AMPLIHACK_AGENTS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_agent_is_recognised() {
        // The agent at the heart of issue #838.
        assert!(is_amplihack_agent("prompt-writer"));
        // A handful of other agents drawn from the three subdirectories.
        assert!(is_amplihack_agent("architect"));
        assert!(is_amplihack_agent("builder"));
        assert!(is_amplihack_agent("ambiguity"));
        assert!(is_amplihack_agent("amplihack-improvement-workflow"));
    }

    #[test]
    fn unknown_agent_is_rejected() {
        assert!(!is_amplihack_agent("nonexistent-agent"));
        assert!(!is_amplihack_agent(""));
        assert!(!is_amplihack_agent("PROMPT-WRITER")); // case-sensitive
    }

    #[test]
    fn agent_only_names_are_not_confused_with_skill_only_names() {
        // `prompt-writer` and `guide` exist *only* as agents (no SKILL.md),
        // so they must be present in this registry.
        assert!(is_amplihack_agent("prompt-writer"));
        assert!(is_amplihack_agent("guide"));

        // `default-workflow` is a skill, not an agent, so it must be absent.
        assert!(!is_amplihack_agent("default-workflow"));
        assert!(!is_amplihack_agent("pdf"));
    }

    #[test]
    fn dual_skill_and_agent_names_are_present() {
        // `gherkin-expert` and `tla-plus-expert` exist as BOTH a skill and an
        // agent. They must still appear in the agent registry; the redirect
        // guard in `pre_tool_use` resolves the overlap by giving skills
        // precedence.
        assert!(is_amplihack_agent("gherkin-expert"));
        assert!(is_amplihack_agent("tla-plus-expert"));
    }

    #[test]
    fn agent_count_matches_unique_bundled_agents() {
        // 42 agent `.md` files, but `guide` is duplicated across
        // `agents/` and `agents/core/`, leaving 41 unique names.
        assert_eq!(agent_count(), 41);
    }

    #[test]
    fn all_agents_are_kebab_case() {
        for agent in AMPLIHACK_AGENTS.iter() {
            assert!(
                agent
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "unexpected characters in agent name: {agent}"
            );
        }
    }

    #[test]
    fn agents_are_sorted_for_binary_search() {
        for pair in AMPLIHACK_AGENTS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "agent list must stay sorted: {} before {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn no_duplicate_agent_names() {
        use std::collections::BTreeSet;
        let unique: BTreeSet<_> = AMPLIHACK_AGENTS.iter().collect();
        assert_eq!(
            unique.len(),
            AMPLIHACK_AGENTS.len(),
            "agent registry must not contain duplicates"
        );
    }

    /// Compile-time-style consistency check: the registry must exactly match
    /// the set of unique agent `.md` basenames bundled under
    /// `amplifier-bundle/agents/`. This fails the build when an agent is added,
    /// removed, or renamed without updating the registry (registry drift).
    #[test]
    fn registry_matches_bundled_agent_files() {
        use std::collections::BTreeSet;
        use std::fs;
        use std::path::{Path, PathBuf};

        fn collect_agent_files(dir: &Path, files: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read bundled agents dir") {
                let path = entry.expect("read bundled agents entry").path();
                if path.is_dir() {
                    collect_agent_files(&path, files);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                    files.push(path);
                }
            }
        }

        let agents_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("amplifier-bundle/agents");
        let mut agent_files = Vec::new();
        collect_agent_files(&agents_dir, &mut agent_files);

        let bundled: BTreeSet<_> = agent_files
            .iter()
            .map(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .expect("agent file basename")
                    .to_string()
            })
            .collect();
        let registered: BTreeSet<_> = AMPLIHACK_AGENTS
            .iter()
            .map(|agent| (*agent).to_string())
            .collect();

        assert_eq!(
            bundled, registered,
            "known agent registry must match bundled agent .md basenames"
        );
    }
}
