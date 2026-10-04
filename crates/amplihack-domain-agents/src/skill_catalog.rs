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
    /// The caller-selected root (including a root symlink) establishes the trust
    /// boundary when opened. Descendants are opened relative to directory handles
    /// without following links. This is not a snapshot of concurrent writes.
    pub fn load(skills_dir: &Path) -> Result<Self> {
        Self::load_with_hook(skills_dir, &mut |_| {})
    }

    #[cfg(unix)]
    fn load_with_hook(root: &Path, before_open: &mut impl FnMut(&Path)) -> Result<Self> {
        use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, statat};
        use std::os::unix::ffi::OsStrExt;
        fn visit(
            dir: &std::fs::File,
            display: &Path,
            skills: &mut HashMap<String, Skill>,
            before_open: &mut impl FnMut(&Path),
        ) -> Result<()> {
            let entries = Dir::read_from(dir).map_err(|e| io_error(display, e))?;
            let mut names = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| io_error(display, e))?;
                let name = entry.file_name().to_bytes();
                if name != b"." && name != b".." {
                    names.push(std::ffi::OsStr::from_bytes(name).to_owned());
                }
            }
            names.sort();
            for name in names {
                let path = display.join(&name);
                let stat = statat(dir, &name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|e| io_error(&path, e))?;
                let kind = FileType::from_raw_mode(stat.st_mode);
                if kind == FileType::Directory {
                    before_open(&path);
                    let child = openat(
                        dir,
                        &name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|e| io_error(&path, e))?;
                    visit(&std::fs::File::from(child), &path, skills, before_open)?;
                } else if name == "SKILL.md" {
                    if kind != FileType::RegularFile {
                        return Err(DomainError::InvalidInput(format!(
                            "skill must be an ordinary file: {}",
                            path.display()
                        )));
                    }
                    before_open(&path);
                    let fd = openat(
                        dir,
                        &name,
                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|e| io_error(&path, e))?;
                    let file = std::fs::File::from(fd);
                    if !file.metadata().map_err(|e| io_error(&path, e))?.is_file() {
                        return Err(DomainError::InvalidInput(format!(
                            "skill must be an ordinary file: {}",
                            path.display()
                        )));
                    }
                    let skill = load_skill(&path, file)?;
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
        let fd = open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| io_error(root, e))?;
        let mut skills = HashMap::new();
        visit(&std::fs::File::from(fd), root, &mut skills, before_open)?;
        Ok(Self { skills })
    }

    #[cfg(not(unix))]
    fn load_with_hook(root: &Path, _: &mut impl FnMut(&Path)) -> Result<Self> {
        Err(DomainError::InvalidInput(format!(
            "secure skill traversal is unsupported on this platform: {}",
            root.display()
        )))
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

fn io_error(path: &Path, error: impl std::fmt::Display) -> DomainError {
    DomainError::InvalidInput(format!("cannot read {}: {error}", path.display()))
}

/// Parse a single SKILL.md from its validated, already opened handle.
fn load_skill(path: &Path, mut file: std::fs::File) -> Result<Skill> {
    use std::io::Read;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| io_error(path, e))?;

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
            "token_budget: -1",
            "token_budget: 4294967296",
            "token_budget: 1.5",
            "token_budget: {total: -1}",
            "token_budget: {total: 4294967296}",
            "token_budget: {total: 1.5}",
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
    fn catalog_reports_traversal_failures_even_when_privileged() {
        let dir = tempfile::tempdir().unwrap();
        // Missing paths and ordinary files fail read_dir even when permission
        // checks would be bypassed by a privileged test runner.
        let missing = dir.path().join("missing");
        let file = dir.path().join("ordinary-file");
        std::fs::write(&file, "not a directory").unwrap();
        for path in [missing, file] {
            let error = SkillCatalog::load(&path).unwrap_err().to_string();
            assert!(error.contains("cannot read"), "{error}");
            assert!(error.contains(&path.display().to_string()), "{error}");
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

    #[cfg(unix)]
    #[test]
    fn replacements_between_inspection_and_open_cannot_follow_links() {
        for directory in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(
                outside.path().join("SKILL.md"),
                "---\nname: outside\n---\nOutside sentinel",
            )
            .unwrap();
            let target = if directory {
                root.path().join("nested")
            } else {
                root.path().join("SKILL.md")
            };
            if directory {
                std::fs::create_dir(&target).unwrap();
            } else {
                std::fs::write(&target, "---\nname: inside\n---\nInside").unwrap();
            }
            let error = SkillCatalog::load_with_hook(root.path(), &mut |path| {
                if path == target {
                    if directory {
                        std::fs::remove_dir(path).unwrap();
                    } else {
                        std::fs::remove_file(path).unwrap();
                    }
                    let destination = if directory {
                        outside.path().to_path_buf()
                    } else {
                        outside.path().join("SKILL.md")
                    };
                    std::os::unix::fs::symlink(destination, path).unwrap();
                }
            })
            .unwrap_err();
            assert!(error.to_string().contains(&target.display().to_string()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn root_link_is_allowed_and_replaced_special_file_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("root-link");
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(SkillCatalog::load(&link).unwrap().is_empty());
        let skill = source.join("SKILL.md");
        std::fs::write(&skill, "---\nname: inside\n---\nInside").unwrap();
        assert!(
            SkillCatalog::load_with_hook(&source, &mut |path| {
                std::fs::remove_file(path).unwrap();
                std::fs::create_dir(path).unwrap();
            })
            .unwrap_err()
            .to_string()
            .contains("ordinary file")
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
