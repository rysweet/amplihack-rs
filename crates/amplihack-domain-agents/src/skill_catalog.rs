//! Skill catalog: loads and indexes SKILL.md files from the amplifier-bundle.
//!
//! Each skill directory under `amplifier-bundle/skills/` contains a `SKILL.md`
//! with YAML front-matter (name, description, auto_activates, etc.) followed
//! by Markdown prompt content. This module parses that structure into a
//! queryable catalog.

use std::borrow::Cow;
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
    pub token_budget: Option<TokenBudget>,
    #[serde(flatten)]
    pub extensions: std::collections::BTreeMap<String, serde_yaml::Value>,
}

/// Resource budget metadata retained without reducing mappings to a total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TokenBudget {
    Scalar(u32),
    Resources(ResourceBudget),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub skill_md: Option<u32>,
    pub reference_md: Option<u32>,
    pub examples_md: Option<u32>,
    pub patterns_md: Option<u32>,
    pub total: Option<u32>,
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
    /// Recursively load ordinary skill files, rejecting incomplete catalogs.
    pub fn load(skills_dir: &Path) -> Result<Self> {
        fn visit(dir: &Path, skills: &mut HashMap<String, Skill>) -> Result<()> {
            let entries = std::fs::read_dir(dir).map_err(|e| {
                DomainError::InvalidInput(format!("cannot read {}: {e}", dir.display()))
            })?;
            let mut paths = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| {
                    DomainError::InvalidInput(format!(
                        "cannot read entry in {}: {e}",
                        dir.display()
                    ))
                })?;
                paths.push(entry.path());
            }
            paths.sort();
            for path in paths {
                let kind = std::fs::symlink_metadata(&path)
                    .map_err(|e| {
                        DomainError::InvalidInput(format!("cannot inspect {}: {e}", path.display()))
                    })?
                    .file_type();
                if kind.is_dir() {
                    visit(&path, skills)?;
                } else if path.file_name().is_some_and(|name| name == "SKILL.md") {
                    if !kind.is_file() {
                        return Err(DomainError::InvalidInput(format!(
                            "skill must be an ordinary file: {}",
                            path.display()
                        )));
                    }
                    let skill = load_skill(&path)?;
                    if let Some(previous) = skills.get(&skill.meta.name) {
                        return Err(DomainError::InvalidInput(format!(
                            "duplicate skill {}: {} and {}",
                            skill.meta.name,
                            previous.path.display(),
                            path.display()
                        )));
                    }
                    skills.insert(skill.meta.name.clone(), skill);
                }
            }
            Ok(())
        }
        let mut skills = HashMap::new();
        visit(skills_dir, &mut skills)?;
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
    // Most bundled files use LF; only CRLF input needs a normalized copy.
    let content = if content.contains("\r\n") {
        Cow::Owned(content.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(content)
    };
    if !content.starts_with("---\n") {
        return Err(DomainError::InvalidInput(format!(
            "no YAML front-matter delimiter in {}",
            source_path.display()
        )));
    }
    let after_first = &content[3..];
    let end = after_first
        .match_indices("\n---")
        .find(|(i, _)| {
            let tail = &after_first[i + 4..];
            tail.is_empty() || tail.starts_with('\n')
        })
        .map(|(i, _)| i)
        .ok_or_else(|| {
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
    if meta.name.is_empty()
        || meta.name.split('-').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return Err(DomainError::InvalidInput(format!(
            "invalid skill name in {}",
            source_path.display()
        )));
    }
    if body.trim().is_empty() {
        return Err(DomainError::InvalidInput(format!(
            "empty skill body in {}",
            source_path.display()
        )));
    }
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
        assert_eq!(meta.token_budget, Some(TokenBudget::Scalar(3000)));
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
    fn metadata_extensions_and_optional_description_round_trip() {
        let text = "---\nname: extended\nargument-hint: '[path]'\nallowed-tools: [Read, Bash]\nmetadata: {category: review}\ntoken_budget: {skill_md: 800}\n---\nBody";
        let (meta, _) = parse_front_matter(text, Path::new("extended.md")).unwrap();
        assert!(meta.description.is_none());
        assert_eq!(meta.extensions["argument-hint"].as_str(), Some("[path]"));
        let serialized = serde_yaml::to_string(&meta).unwrap();
        let reloaded: SkillMeta = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(meta.extensions, reloaded.extensions);
        assert_eq!(meta.token_budget, reloaded.token_budget);
    }

    #[test]
    fn invalid_metadata_and_empty_bodies_fail() {
        for yaml in [
            "token_budget: {total: -1}",
            "token_budget: {total: bad}",
            "confirmation_required: maybe",
            "auto_activates: text",
            "description: []",
        ] {
            let text = format!("---\nname: test\n{yaml}\n---\nBody");
            assert!(
                parse_front_matter(&text, Path::new("bad.md")).is_err(),
                "{yaml}"
            );
        }
        for text in [
            "---invalid\nname: test\n---\nBody",
            "---\nname: test\n---invalid\nBody",
            "---\nname: test\n---\n  ",
        ] {
            assert!(parse_front_matter(text, Path::new("bad.md")).is_err());
        }
    }

    #[test]
    fn catalog_rejects_duplicates_and_parse_failures() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "---\nname: same\n---\nBody").unwrap();
        let nested = dir.path().join("nested/SKILL.md");
        std::fs::write(&nested, "---\nname: same\n---\nBody").unwrap();
        let error = SkillCatalog::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("duplicate") && error.contains("nested"));
        std::fs::write(&nested, "invalid").unwrap();
        let error = SkillCatalog::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("nested/SKILL.md"));
    }

    #[cfg(unix)]
    #[test]
    fn catalog_skips_directory_links_and_rejects_linked_skills() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("cycle")).unwrap();
        assert!(SkillCatalog::load(dir.path()).unwrap().is_empty());
        let source = dir.path().join("source.md");
        std::fs::write(&source, "---\nname: linked\n---\nBody").unwrap();
        std::os::unix::fs::symlink(source, dir.path().join("SKILL.md")).unwrap();
        assert!(
            SkillCatalog::load(dir.path())
                .unwrap_err()
                .to_string()
                .contains("ordinary file")
        );
        let (meta, body) =
            parse_front_matter("---\r\nname: crlf\r\n---\r\nBody", Path::new("crlf.md")).unwrap();
        assert_eq!(meta.name, "crlf");
        assert_eq!(body, "Body");
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
