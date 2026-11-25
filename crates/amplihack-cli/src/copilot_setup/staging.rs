//! Asset staging — agents, skills, command docs, and plugin registration.

use amplihack_types::workflow;
use amplihack_utils::list_selection::{ListSelection, ListSet};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use super::{fs_helpers, jsonc};

pub(super) fn stage_agents(
    source_agents: &Path,
    copilot_home: &Path,
    selection: &ListSelection,
) -> Result<usize> {
    let source_agents = if source_agents.join("amplihack").is_dir() {
        source_agents.join("amplihack")
    } else {
        source_agents.to_path_buf()
    };
    let dest = copilot_home.join("agents").join("amplihack");
    let files = fs_helpers::walk_files(&source_agents)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter(|path| {
            let identity = path
                .strip_prefix(&source_agents)
                .expect("walked source")
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            selection.includes_agent(&identity)
        })
        .collect::<Vec<_>>();
    fs_helpers::reset_markdown_dir(&dest)?;
    for path in &files {
        let leaf = path.file_name().context("agent file missing name")?;
        // Full mode preserves the previous flat layout. In default mode,
        // qualified extensions can expose both agents that share a leaf name.
        let target = if selection.set != ListSet::All
            && files
                .iter()
                .filter(|other| other.file_name() == Some(leaf))
                .count()
                > 1
        {
            dest.join(
                path.strip_prefix(&source_agents)?
                    .to_string_lossy()
                    .replace(['/', '\\'], "--"),
            )
        } else {
            dest.join(leaf)
        };
        fs::copy(path, target)?;
    }
    Ok(files.len())
}

pub(super) fn stage_context(source_context: &Path, copilot_home: &Path) -> Result<usize> {
    let dest = copilot_home.join("context").join("amplihack");
    fs_helpers::reset_markdown_dir(&dest)?;
    fs::create_dir_all(&dest)?;

    let mut count = 0;
    for entry in fs::read_dir(source_context)
        .with_context(|| format!("read context dir {}", source_context.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }

        let target = dest.join(entry.file_name());
        let content = fs::read_to_string(&path)
            .with_context(|| format!("read context file {}", path.display()))?;
        let content = if entry.file_name() == "USER_PREFERENCES.md" {
            canonicalize_user_preferences(&content)
        } else {
            content
        };
        fs::write(&target, content)
            .with_context(|| format!("write context file {}", target.display()))?;
        count += 1;
    }

    Ok(count)
}

fn canonicalize_user_preferences(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if is_stale_selected_workflow_line(line) {
                format!("**Selected**: {}", workflow::DEFAULT_WORKFLOW_SELECTION)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_stale_selected_workflow_line(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    lowered.contains("**selected**")
        && (lowered.contains("default_workflow")
            || lowered.contains("default workflow")
            || lowered.contains(".claude/workflow/")
            || lowered.contains(".claude/workflows/"))
}

pub(super) fn stage_skills(
    source_skills: &Path,
    copilot_home: &Path,
    selection: &ListSelection,
) -> Result<crate::claude_plugin::PluginRegistration> {
    if !source_skills.is_dir() {
        return Ok(crate::claude_plugin::PluginRegistration::default());
    }

    let record = copilot_home.join("copilot-skills-manifest.json");
    let legacy = if selection.set == ListSet::All || !record.try_exists()? {
        legacy_skill_tree(source_skills)?
    } else {
        tempfile::tempdir()?
    };
    if !record.try_exists()? {
        // Fallback for a legacy launch that does not replace the staged
        // definitions. Upgrade paths adopt against the previous payload first.
        crate::claude_plugin::adopt_legacy_skills(
            legacy.path(),
            &copilot_home.join("skills"),
            &record,
        )?;
    }
    if selection.set == ListSet::All {
        let skills = fs::read_dir(legacy.path())?
            .map(|entry| {
                let entry = entry?;
                Ok((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        return crate::claude_plugin::sync_skill_folders(
            legacy.path(),
            &copilot_home.join("skills"),
            &record,
            skills,
        );
    }
    crate::claude_plugin::sync_canonical_skills(
        source_skills,
        &copilot_home.join("skills"),
        &record,
        selection,
    )
}

/// Record byte-identical legacy copies while the previous staged payload is
/// still available. Never infer ownership from a matching folder name alone.
pub(super) fn adopt_previous_skill_ownership(
    source_skills: &Path,
    copilot_home: &Path,
) -> Result<()> {
    let record = copilot_home.join("copilot-skills-manifest.json");
    if record.try_exists()? || !source_skills.is_dir() || !copilot_home.join("skills").is_dir() {
        return Ok(());
    }
    let legacy = legacy_skill_tree(source_skills)?;
    crate::claude_plugin::adopt_legacy_skills(legacy.path(), &copilot_home.join("skills"), &record)
}

fn legacy_skill_tree(source_skills: &Path) -> Result<tempfile::TempDir> {
    let legacy = tempfile::tempdir()?;
    // Older releases copied only Markdown, flattening each top-level folder.
    for entry in fs::read_dir(source_skills)? {
        let entry = entry?;
        if entry.path().is_dir() {
            let expected = legacy.path().join(entry.file_name());
            fs::create_dir_all(&expected)?;
            fs_helpers::flatten_markdown_tree(&entry.path(), &expected)?;
        }
    }
    Ok(legacy)
}

pub(super) fn stage_command_docs(source_commands: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_commands.is_dir() {
        return Ok(0);
    }

    let dest = copilot_home.join("commands").join("amplihack");
    fs_helpers::reset_markdown_dir(&dest)?;

    let mut count = 0;
    for entry in fs::read_dir(source_commands)
        .with_context(|| format!("read commands dir {}", source_commands.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            let target = dest.join(entry.file_name());
            fs::copy(&path, &target)
                .with_context(|| format!("copy {} → {}", path.display(), target.display()))?;
            count += 1;
        }
    }

    Ok(count)
}

pub(super) fn register_plugin(
    source_commands: &Path,
    copilot_home: &Path,
    user_config: super::UserConfig,
) -> Result<bool> {
    let manifest = source_commands.join("plugin.json");
    if !manifest.is_file() {
        return Ok(false);
    }
    let raw_manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest)
            .with_context(|| format!("read plugin manifest {}", manifest.display()))?,
    )
    .with_context(|| format!("parse plugin manifest {}", manifest.display()))?;

    let plugin_name = raw_manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("amplihack");

    let install_dir = copilot_home
        .join("installed-plugins")
        .join(format!("{plugin_name}@local"));
    if install_dir.exists() {
        fs::remove_dir_all(&install_dir)
            .with_context(|| format!("clean old plugin install {}", install_dir.display()))?;
    }
    let commands_dest = install_dir.join("commands");
    fs::create_dir_all(&commands_dest)?;

    for entry in fs::read_dir(source_commands)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            let target = commands_dest.join(entry.file_name());
            fs::copy(&path, &target)?;
        }
    }

    let plugin_meta = json!({
        "name": plugin_name,
        "version": "local",
        "source": "local",
        "commands": commands_dest.display().to_string(),
    });
    fs::write(
        install_dir.join("plugin.json"),
        serde_json::to_string_pretty(&plugin_meta)?,
    )?;

    // Issue #1526: the plugin directory above is amplihack's; config.json is
    // the user's, and an implicit run leaves it alone.
    if user_config == super::UserConfig::KeepExisting {
        return Ok(true);
    }
    let config_path = copilot_home.join("config.json");
    let (mut config, prefix): (Value, String) = if config_path.is_file() {
        let raw = fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?;
        let prefix = jsonc::leading_comment_prefix(&raw).to_string();
        let stripped = jsonc::strip_jsonc_comments(&raw);
        let value = if stripped.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&stripped)
                .with_context(|| format!("parse {}", config_path.display()))?
        };
        (value, prefix)
    } else {
        (json!({}), String::new())
    };

    let plugins = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("config.json is not an object"))?
        .entry("plugins")
        .or_insert_with(|| json!([]));

    let arr = plugins
        .as_array_mut()
        .ok_or_else(|| anyhow!("plugins is not an array"))?;

    // An existing entry is left as it is: rewriting the file to change
    // nothing would only re-format the user's config.
    if arr
        .iter()
        .any(|p| p.get("name").and_then(Value::as_str) == Some(plugin_name))
    {
        return Ok(true);
    }
    arr.push(json!({
        "name": plugin_name,
        "version": "local",
        "installed_path": install_dir.display().to_string(),
    }));

    let body = serde_json::to_string_pretty(&config)?;
    fs::write(&config_path, jsonc::apply_prefix(&prefix, body))
        .with_context(|| format!("write {}", config_path.display()))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #860: staging must copy the `pr-guide` skill into the Copilot
    /// skills tree so Copilot CLI lists it. This exercises the production
    /// `stage_skills` against a pr-guide-shaped source (a canonical `SKILL.md`
    /// with valid `name: pr-guide` frontmatter, a sibling `reference.md`, and a
    /// nested `tests/` dir with a non-markdown file) alongside other skills, and
    /// asserts the discoverable `skills/pr-guide/SKILL.md` lands intact with no
    /// co-located skill dropped.
    #[test]
    fn stage_skills_makes_pr_guide_discoverable_for_copilot() {
        let temp = tempfile::tempdir().unwrap();
        let source_skills = temp.path().join("source-skills");
        let copilot_home = temp.path().join(".copilot");

        // pr-guide, shaped like the real bundled skill.
        let pr_guide = source_skills.join("pr-guide");
        fs::create_dir_all(pr_guide.join("tests")).unwrap();
        fs::write(
            pr_guide.join("SKILL.md"),
            "---\nname: pr-guide\ndescription: Illustrated PR walkthrough.\n---\n\n# PR Guide\n",
        )
        .unwrap();
        fs::write(pr_guide.join("reference.md"), "# Reference\n").unwrap();
        fs::write(
            pr_guide.join("tests").join("test_skill_structure.sh"),
            "#!/usr/bin/env bash\n",
        )
        .unwrap();

        // A co-located skill, to prove staging pr-guide does not drop siblings.
        let other = source_skills.join("pr-review-assistant");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("SKILL.md"),
            "---\nname: pr-review-assistant\ndescription: Review helper.\n---\n\n# Review\n",
        )
        .unwrap();

        let report = stage_skills(
            &source_skills,
            &copilot_home,
            &ListSelection::new(ListSet::All, []),
        )
        .unwrap();

        let staged_pr_guide = copilot_home
            .join("skills")
            .join("pr-guide")
            .join("SKILL.md");
        assert!(
            staged_pr_guide.is_file(),
            "pr-guide/SKILL.md must be staged to {} so Copilot CLI lists the skill",
            staged_pr_guide.display()
        );

        let staged_body = fs::read_to_string(&staged_pr_guide).unwrap();
        assert!(
            staged_body.starts_with("---\n") && staged_body.contains("name: pr-guide"),
            "staged pr-guide/SKILL.md must preserve valid `name: pr-guide` frontmatter"
        );
        assert!(
            copilot_home
                .join("skills")
                .join("pr-guide")
                .join("reference.md")
                .is_file(),
            "pr-guide supporting markdown must be staged alongside SKILL.md"
        );
        assert!(
            copilot_home
                .join("skills")
                .join("pr-review-assistant")
                .join("SKILL.md")
                .is_file(),
            "staging pr-guide must not drop co-located skills"
        );
        assert!(
            report.published.len() == 2,
            "expected both skills to be published, got {report:?}"
        );
    }

    /// Issue #3220: the bundled `skill-and-agents-directory` skill reaches the
    /// Copilot skills tree through the existing staging, with no code of its own.
    #[test]
    fn stage_skills_stages_the_bundled_directory_skill_for_copilot() {
        let temp = tempfile::tempdir().unwrap();
        let copilot_home = temp.path().join(".copilot");
        let bundle_skills =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");

        stage_skills(&bundle_skills, &copilot_home, &ListSelection::default()).unwrap();

        let staged = copilot_home
            .join("skills")
            .join("skill-and-agents-directory")
            .join("SKILL.md");
        let body = fs::read_to_string(&staged).unwrap();
        assert!(body.contains("name: skill-and-agents-directory"));
    }

    /// Issue #860: re-staging must not duplicate or lose the `pr-guide` skill —
    /// `reset_markdown_dir` clears stale markdown before each copy, so a second
    /// stage yields exactly the same discoverable SKILL.md.
    #[test]
    fn stage_skills_is_idempotent_for_pr_guide() {
        let temp = tempfile::tempdir().unwrap();
        let source_skills = temp.path().join("source-skills");
        let copilot_home = temp.path().join(".copilot");

        let pr_guide = source_skills.join("pr-guide");
        fs::create_dir_all(&pr_guide).unwrap();
        fs::write(
            pr_guide.join("SKILL.md"),
            "---\nname: pr-guide\ndescription: Illustrated PR walkthrough.\n---\n",
        )
        .unwrap();

        stage_skills(
            &source_skills,
            &copilot_home,
            &ListSelection::new(ListSet::All, []),
        )
        .unwrap();
        stage_skills(
            &source_skills,
            &copilot_home,
            &ListSelection::new(ListSet::All, []),
        )
        .unwrap();

        let staged_dir = copilot_home.join("skills").join("pr-guide");
        let markdown: Vec<_> = fs::read_dir(&staged_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "md"))
            .collect();
        assert_eq!(
            markdown,
            vec![std::ffi::OsString::from("SKILL.md")],
            "re-staging pr-guide must leave exactly one discoverable SKILL.md"
        );
    }

    #[test]
    fn staging_leaves_an_unowned_legacy_named_file_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let copilot_home = temp.path().join(".copilot");
        let legacy_parent = copilot_home.join("skills");
        fs::create_dir_all(&legacy_parent).unwrap();
        let legacy_path = legacy_parent.join("azure-devops-cli");
        fs::write(&legacy_path, "user content").unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let report = stage_skills(&source, &copilot_home, &ListSelection::default()).unwrap();
        assert!(report.removed_skills.is_empty());
        assert!(report.preserved.is_empty());
        assert_eq!(fs::read(&legacy_path).unwrap(), b"user content");
    }
}
