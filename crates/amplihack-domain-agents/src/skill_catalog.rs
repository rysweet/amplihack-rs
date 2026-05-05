//! Skill catalog: loads and indexes SKILL.md files from the amplifier-bundle.
//!
//! Each skill directory under `amplifier-bundle/skills/` contains a `SKILL.md`
//! with YAML front-matter (name, description, auto_activates, etc.) followed
//! by Markdown prompt content. This module parses that structure into a
//! queryable catalog.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

/// Metadata parsed from YAML front-matter in SKILL.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub auto_activates: Vec<String>,
    #[serde(default)]
    pub explicit_triggers: Vec<String>,
    #[serde(default)]
    pub confirmation_required: bool,
    #[serde(default)]
    pub skip_confirmation_if_explicit: bool,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

/// A fully loaded skill: metadata + prompt content.
#[derive(Debug, Clone)]
pub struct Skill {
    pub meta: SkillMeta,
    /// Markdown body after the YAML front-matter.
    pub prompt: String,
    /// Directory this skill was loaded from.
    pub path: PathBuf,
}

/// Indexed collection of all bundled skills.
#[derive(Debug, Clone)]
pub struct SkillCatalog {
    skills: HashMap<String, Skill>,
}

impl SkillCatalog {
    /// Load every skill directory under `skills_dir`.
    ///
    /// Each subdirectory must contain a `SKILL.md` file. Directories without
    /// one are silently skipped.
    pub fn load(skills_dir: &Path) -> Result<Self> {
        let mut skills = HashMap::new();

        let entries = std::fs::read_dir(skills_dir).map_err(|e| {
            DomainError::InvalidInput(format!(
                "cannot read skills directory {}: {e}",
                skills_dir.display()
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                DomainError::InvalidInput(format!("error reading directory entry: {e}"))
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            match load_skill(&skill_md) {
                Ok(skill) => {
                    skills.insert(skill.meta.name.clone(), skill);
                }
                Err(e) => {
                    tracing::warn!(
                        path = %skill_md.display(),
                        error = %e,
                        "skipping skill with parse error"
                    );
                }
            }
        }

        Ok(Self { skills })
    }

    /// Number of loaded skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// All skill names, sorted alphabetically.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.skills.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// Find skills whose `auto_activates` patterns match the given text.
    pub fn match_auto_activate(&self, text: &str) -> Vec<&Skill> {
        let lower = text.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.meta
                    .auto_activates
                    .iter()
                    .any(|pattern| lower.contains(&pattern.to_lowercase()))
            })
            .collect()
    }

    /// Find skills by explicit trigger (e.g. `/amplihack:default-workflow`).
    pub fn find_by_trigger(&self, trigger: &str) -> Option<&Skill> {
        self.skills
            .values()
            .find(|s| s.meta.explicit_triggers.iter().any(|t| t == trigger))
    }

    /// Iterator over all skills.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Skill)> {
        self.skills.iter()
    }
}

/// Parse a single SKILL.md file into a `Skill`.
fn load_skill(path: &Path) -> Result<Skill> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| DomainError::InvalidInput(format!("cannot read {}: {e}", path.display())))?;

    let (meta, prompt) = parse_front_matter(&content, path)?;

    let dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    Ok(Skill {
        meta,
        prompt,
        path: dir,
    })
}

/// Split content into YAML front-matter and Markdown body.
///
/// Expects the file to start with `---\n`, followed by YAML, then `---\n`.
fn parse_front_matter(content: &str, source_path: &Path) -> Result<(SkillMeta, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return Err(DomainError::InvalidInput(format!(
            "no YAML front-matter delimiter in {}",
            source_path.display()
        )));
    }
    let after_first = &content[3..];
    let end = after_first.find("\n---").ok_or_else(|| {
        DomainError::InvalidInput(format!(
            "unclosed YAML front-matter in {}",
            source_path.display()
        ))
    })?;
    let yaml_str = &after_first[..end];
    let body_start = end + 4; // skip "\n---"
    let body = if body_start < after_first.len() {
        after_first[body_start..]
            .trim_start_matches('\n')
            .to_string()
    } else {
        String::new()
    };

    let meta: SkillMeta = serde_yaml::from_str(yaml_str).map_err(|e| {
        DomainError::InvalidInput(format!("invalid YAML in {}: {e}", source_path.display()))
    })?;
    Ok((meta, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_front_matter_basic() {
        let content = "---\nname: test-skill\ndescription: A test\n---\n# Body\nHello";
        let (meta, body) = parse_front_matter(content, Path::new("test.md")).unwrap();
        assert_eq!(meta.name, "test-skill");
        assert_eq!(meta.description.as_deref(), Some("A test"));
        assert!(body.starts_with("# Body"));
    }

    #[test]
    fn parse_front_matter_with_lists() {
        let content = r#"---
name: my-skill
auto_activates:
  - "pattern one"
  - "pattern two"
explicit_triggers:
  - /amplihack:my-skill
confirmation_required: true
token_budget: 3000
---
# Prompt
Do stuff."#;
        let (meta, body) = parse_front_matter(content, Path::new("test.md")).unwrap();
        assert_eq!(meta.name, "my-skill");
        assert_eq!(meta.auto_activates.len(), 2);
        assert_eq!(meta.explicit_triggers, vec!["/amplihack:my-skill"]);
        assert!(meta.confirmation_required);
        assert_eq!(meta.token_budget, Some(3000));
        assert!(body.contains("Do stuff."));
    }

    #[test]
    fn parse_front_matter_missing_returns_err() {
        let result = parse_front_matter("# No front matter", Path::new("bad.md"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("bad.md"),
            "error should include filename: {msg}"
        );
    }

    #[test]
    fn parse_front_matter_invalid_yaml_includes_details() {
        let content = "---\n[not: valid: yaml:\n---\nBody";
        let result = parse_front_matter(content, Path::new("broken.md"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("broken.md"),
            "error should include filename: {msg}"
        );
    }

    #[test]
    fn catalog_load_from_bundle() {
        let skills_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
        if !skills_dir.exists() {
            // Skip in CI if bundle not present.
            return;
        }
        let catalog = SkillCatalog::load(&skills_dir).unwrap();
        assert!(
            catalog.len() >= 70,
            "expected ~75+ skills, got {}",
            catalog.len()
        );
        // Spot-check known skills.
        assert!(catalog.get("default-workflow").is_some());
        assert!(catalog.get("azure-admin").is_some());
    }

    #[test]
    fn catalog_names_sorted() {
        let skills_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
        if !skills_dir.exists() {
            return;
        }
        let catalog = SkillCatalog::load(&skills_dir).unwrap();
        let names = catalog.names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn catalog_match_auto_activate() {
        let skills_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
        if !skills_dir.exists() {
            return;
        }
        let catalog = SkillCatalog::load(&skills_dir).unwrap();
        let matches = catalog.match_auto_activate("implement feature spanning multiple files");
        assert!(
            !matches.is_empty(),
            "expected default-workflow to auto-activate"
        );
    }
}
