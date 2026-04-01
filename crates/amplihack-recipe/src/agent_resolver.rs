//! Agent resolver — maps agent references to markdown content.
//!
//! Matches Python `amplihack/recipes/agent_resolver.py`:
//! - Resolves `namespace:name` references (e.g. `amplihack:builder`)
//! - Searches configurable list of directories
//! - Path traversal prevention via safe-name regex

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tracing::debug;

/// Error when an agent reference cannot be resolved.
#[derive(Debug, thiserror::Error)]
#[error("Agent '{agent_ref}' not found.{}", searched_msg(.searched))]
pub struct AgentNotFoundError {
    pub agent_ref: String,
    pub searched: Vec<String>,
}

fn searched_msg(paths: &[String]) -> String {
    if paths.is_empty() {
        String::new()
    } else {
        format!(" Searched: {}", paths.join(", "))
    }
}

/// Default search paths for agent definitions.
fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        paths.push(home.join(".amplihack/.claude/agents"));
    }
    paths.push(PathBuf::from(".claude/agents"));
    paths.push(PathBuf::from("amplifier-bundle/agents"));
    paths.push(PathBuf::from("src/amplihack/amplifier-bundle/agents"));
    paths.push(PathBuf::from("src/amplihack/.claude/agents"));
    paths
}

/// Resolves `namespace:name` agent references to their markdown content.
pub struct AgentResolver {
    search_paths: Vec<PathBuf>,
}

impl AgentResolver {
    pub fn new(search_paths: Option<Vec<PathBuf>>) -> Self {
        Self {
            search_paths: search_paths.unwrap_or_else(default_search_paths),
        }
    }

    /// Resolve an agent reference to its markdown content.
    ///
    /// The reference format is `namespace:name` or `namespace:sub:name`.
    /// Each segment is validated to prevent path traversal.
    pub fn resolve(&self, agent_ref: &str) -> Result<String> {
        let parts: Vec<&str> = agent_ref.split(':').collect();
        if parts.is_empty() || parts.len() > 3 {
            bail!("Invalid agent reference format: '{agent_ref}'");
        }

        // Validate each segment against path traversal
        for part in &parts {
            if !is_safe_name(part) {
                bail!(
                    "Invalid agent reference segment '{part}' in '{agent_ref}': \
                     only alphanumeric, hyphens, and underscores allowed"
                );
            }
        }

        // Build relative path from reference parts
        let relative = match parts.len() {
            1 => format!("{}.md", parts[0]),
            2 => format!("{}/{}.md", parts[0], parts[1]),
            3 => format!("{}/{}/{}.md", parts[0], parts[1], parts[2]),
            _ => unreachable!(),
        };

        let mut searched = Vec::new();
        for search_dir in &self.search_paths {
            let candidate = search_dir.join(&relative);
            searched.push(candidate.display().to_string());
            if candidate.is_file() {
                debug!(path = %candidate.display(), "Resolved agent reference");
                return std::fs::read_to_string(&candidate)
                    .with_context(|| format!("failed to read agent file: {}", candidate.display()));
            }
        }

        Err(AgentNotFoundError {
            agent_ref: agent_ref.to_string(),
            searched,
        }
        .into())
    }

    /// List all discoverable agent references.
    pub fn list_agents(&self) -> Vec<String> {
        let mut agents = Vec::new();
        for search_dir in &self.search_paths {
            if let Ok(entries) = std::fs::read_dir(search_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "md") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            agents.push(stem.to_string());
                        }
                    }
                }
            }
        }
        agents.sort();
        agents.dedup();
        agents
    }
}

/// Check if a name segment is safe (no path traversal).
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_validation() {
        assert!(is_safe_name("builder"));
        assert!(is_safe_name("core-architect"));
        assert!(is_safe_name("my_agent"));
        assert!(!is_safe_name(".."));
        assert!(!is_safe_name("path/traversal"));
        assert!(!is_safe_name(""));
        assert!(!is_safe_name("has space"));
    }

    #[test]
    fn resolve_single_segment() {
        let dir = tempfile::tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("builder.md"), "# Builder Agent").unwrap();

        let resolver = AgentResolver::new(Some(vec![agents_dir]));
        let content = resolver.resolve("builder").unwrap();
        assert_eq!(content, "# Builder Agent");
    }

    #[test]
    fn resolve_two_segment() {
        let dir = tempfile::tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        let ns_dir = agents_dir.join("amplihack");
        std::fs::create_dir_all(&ns_dir).unwrap();
        std::fs::write(ns_dir.join("builder.md"), "# amplihack Builder").unwrap();

        let resolver = AgentResolver::new(Some(vec![agents_dir]));
        let content = resolver.resolve("amplihack:builder").unwrap();
        assert_eq!(content, "# amplihack Builder");
    }

    #[test]
    fn resolve_rejects_path_traversal() {
        let resolver = AgentResolver::new(Some(vec![]));
        let result = resolver.resolve("../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_returns_error_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = AgentResolver::new(Some(vec![dir.path().to_path_buf()]));
        let result = resolver.resolve("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn list_agents_finds_md_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("builder.md"), "content").unwrap();
        std::fs::write(dir.path().join("reviewer.md"), "content").unwrap();
        std::fs::write(dir.path().join("not-agent.txt"), "content").unwrap();

        let resolver = AgentResolver::new(Some(vec![dir.path().to_path_buf()]));
        let agents = resolver.list_agents();
        assert_eq!(agents, vec!["builder", "reviewer"]);
    }
}
