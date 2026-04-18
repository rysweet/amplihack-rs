//! Agent resolver — maps agent references to markdown content.
//!
//! Matches Python `amplihack/recipes/agent_resolver.py`:
//! - Resolves `namespace:name` references (e.g. `amplihack:builder`)
//! - Searches configurable list of directories
//! - Path traversal prevention via safe-name regex
//! - Parses YAML front-matter metadata from agent definitions
//! - Recursive directory listing for full catalog discovery

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
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

/// Parsed front-matter metadata from an agent markdown file.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentMetadata {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// A fully resolved agent definition: metadata + body content.
#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub metadata: AgentMetadata,
    /// Category path relative to the agents directory (e.g. "core", "specialized").
    pub category: String,
    /// The full markdown content (including front-matter).
    pub content: String,
}

/// Parse YAML front-matter delimited by `---` lines.
/// Returns `(metadata, body)` or `None` if no front-matter is present.
fn parse_front_matter(content: &str) -> Option<(AgentMetadata, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find end delimiter
    let after_first = &trimmed[3..].trim_start_matches(['\r', '\n']);
    let end = after_first.find("\n---")?;
    let yaml_block = &after_first[..end];
    let body_start = end + 4; // skip "\n---"
    let body = after_first.get(body_start..).unwrap_or("");
    let meta: AgentMetadata = serde_yaml::from_str(yaml_block).ok()?;
    Some((meta, body))
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
                return std::fs::read_to_string(&candidate).with_context(|| {
                    format!("failed to read agent file: {}", candidate.display())
                });
            }
        }

        Err(AgentNotFoundError {
            agent_ref: agent_ref.to_string(),
            searched,
        }
        .into())
    }

    /// List all discoverable agent references, recursing into subdirectories.
    ///
    /// Returns references in `category:name` format for agents in subdirectories
    /// (e.g. `core:builder`) and plain `name` for root-level agents.
    pub fn list_agents(&self) -> Vec<String> {
        let mut agents = Vec::new();
        for search_dir in &self.search_paths {
            collect_agents_recursive(search_dir, search_dir, &mut agents);
        }
        agents.sort();
        agents.dedup();
        agents
    }

    /// Build a full catalog of all agents with parsed metadata.
    ///
    /// Walks all search paths recursively, parses front-matter from each
    /// `.md` file, and returns `AgentDefinition` entries.
    pub fn catalog(&self) -> Vec<AgentDefinition> {
        let mut defs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for search_dir in &self.search_paths {
            collect_definitions_recursive(search_dir, search_dir, &mut defs, &mut seen);
        }
        defs.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
        defs
    }

    /// Parse the front-matter metadata from a resolved agent's content.
    pub fn parse_metadata(content: &str) -> Option<AgentMetadata> {
        parse_front_matter(content).map(|(meta, _)| meta)
    }
}

/// Recursively collect agent names from a directory tree.
fn collect_agents_recursive(base: &Path, dir: &Path, agents: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_agents_recursive(base, &path, agents);
        } else if path.extension().is_some_and(|e| e == "md")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            let ref_name = if let Ok(rel) = path.strip_prefix(base) {
                if let Some(parent) = rel.parent().filter(|p| p != &Path::new("")) {
                    format!("{}:{}", parent.display(), stem)
                } else {
                    stem.to_string()
                }
            } else {
                stem.to_string()
            };
            agents.push(ref_name);
        }
    }
}

/// Recursively collect full agent definitions from a directory tree.
fn collect_definitions_recursive(
    base: &Path,
    dir: &Path,
    defs: &mut Vec<AgentDefinition>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_definitions_recursive(base, &path, defs, seen);
        } else if path.extension().is_some_and(|e| e == "md") {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let category = path
                .strip_prefix(base)
                .ok()
                .and_then(|rel| rel.parent())
                .filter(|p| p != &Path::new(""))
                .map(|p| p.display().to_string())
                .unwrap_or_default();

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();

            // Deduplicate by name (first-found wins, matching search path priority)
            if !seen.insert(stem.clone()) {
                continue;
            }

            let metadata = parse_front_matter(&content)
                .map(|(meta, _)| meta)
                .unwrap_or(AgentMetadata {
                    name: stem,
                    version: None,
                    description: None,
                    role: None,
                    model: None,
                });

            defs.push(AgentDefinition {
                metadata,
                category,
                content,
            });
        }
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

    #[test]
    fn list_agents_recurses_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("core");
        let specialized = dir.path().join("specialized");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::create_dir_all(&specialized).unwrap();
        std::fs::write(dir.path().join("guide.md"), "# Guide").unwrap();
        std::fs::write(core.join("builder.md"), "# Builder").unwrap();
        std::fs::write(specialized.join("security.md"), "# Security").unwrap();

        let resolver = AgentResolver::new(Some(vec![dir.path().to_path_buf()]));
        let agents = resolver.list_agents();
        assert_eq!(
            agents,
            vec!["core:builder", "guide", "specialized:security"]
        );
    }

    #[test]
    fn parse_front_matter_extracts_metadata() {
        let content =
            "---\nname: builder\nversion: 1.0.0\ndescription: Builds things\n---\n# Builder";
        let (meta, body) = parse_front_matter(content).unwrap();
        assert_eq!(meta.name, "builder");
        assert_eq!(meta.version.as_deref(), Some("1.0.0"));
        assert_eq!(meta.description.as_deref(), Some("Builds things"));
        assert!(body.contains("# Builder"));
    }

    #[test]
    fn parse_front_matter_returns_none_without_delimiters() {
        let content = "# Just a heading\nNo front-matter here.";
        assert!(parse_front_matter(content).is_none());
    }

    #[test]
    fn catalog_returns_definitions_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("core");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(
            core.join("builder.md"),
            "---\nname: builder\nversion: 1.0.0\ndescription: Builds stuff\nrole: builder\nmodel: inherit\n---\n# Builder Agent\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("guide.md"), "# Guide (no front-matter)\n").unwrap();

        let resolver = AgentResolver::new(Some(vec![dir.path().to_path_buf()]));
        let catalog = resolver.catalog();
        assert_eq!(catalog.len(), 2);

        let builder = catalog
            .iter()
            .find(|d| d.metadata.name == "builder")
            .unwrap();
        assert_eq!(builder.category, "core");
        assert_eq!(builder.metadata.version.as_deref(), Some("1.0.0"));

        let guide = catalog.iter().find(|d| d.metadata.name == "guide").unwrap();
        assert!(guide.category.is_empty());
        assert!(guide.metadata.description.is_none());
    }
}
