//! Asset staging — agents, skills, command docs, and plugin registration.

use amplihack_types::workflow;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use super::{fs_helpers, jsonc};

pub(super) fn stage_agents(source_agents: &Path, copilot_home: &Path) -> Result<usize> {
    let dest = copilot_home.join("agents").join("amplihack");
    fs_helpers::reset_markdown_dir(&dest)?;
    fs_helpers::flatten_markdown_tree(source_agents, &dest)
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

pub(super) fn stage_skills(source_skills: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_skills.is_dir() {
        return Ok(0);
    }

    remove_legacy_cli_skill(copilot_home)?;

    let mut count = 0;
    for entry in fs::read_dir(source_skills)
        .with_context(|| format!("read skills dir {}", source_skills.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_name = entry.file_name();
        let dest = copilot_home.join("skills").join(&skill_name);
        fs_helpers::reset_markdown_dir(&dest)?;
        count += fs_helpers::flatten_markdown_tree(&path, &dest)?;
    }

    Ok(count)
}

fn remove_legacy_cli_skill(copilot_home: &Path) -> Result<()> {
    let legacy_cli_skill = copilot_home.join("skills").join("azure-devops-cli");
    let metadata = match fs::symlink_metadata(&legacy_cli_skill) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("inspect legacy skill path {}", legacy_cli_skill.display())
            });
        }
    };

    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(&legacy_cli_skill)
            .with_context(|| format!("remove legacy skill file {}", legacy_cli_skill.display()))?;
    } else if metadata.is_dir() {
        fs::remove_dir_all(&legacy_cli_skill)
            .with_context(|| format!("remove legacy skill dir {}", legacy_cli_skill.display()))?;
    } else {
        return Err(anyhow!(
            "legacy skill path is not a file, directory, or symlink: {}",
            legacy_cli_skill.display()
        ));
    }

    Ok(())
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

pub(super) fn register_plugin(source_commands: &Path, copilot_home: &Path) -> Result<bool> {
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

    if !arr
        .iter()
        .any(|p| p.get("name").and_then(Value::as_str) == Some(plugin_name))
    {
        arr.push(json!({
            "name": plugin_name,
            "version": "local",
            "installed_path": install_dir.display().to_string(),
        }));
    }

    let body = serde_json::to_string_pretty(&config)?;
    fs::write(&config_path, jsonc::apply_prefix(&prefix, body))
        .with_context(|| format!("write {}", config_path.display()))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn legacy_cli_skill_cleanup_rejects_unsupported_file_type() {
        use std::os::unix::net::UnixListener;

        let temp = tempfile::Builder::new()
            .prefix("ah")
            .tempdir_in("/tmp")
            .unwrap();
        let copilot_home = temp.path().join(".copilot");
        let legacy_parent = copilot_home.join("skills");
        fs::create_dir_all(&legacy_parent).unwrap();
        let legacy_path = legacy_parent.join("azure-devops-cli");
        let _listener = UnixListener::bind(&legacy_path).unwrap();

        let err = remove_legacy_cli_skill(&copilot_home).unwrap_err();

        assert!(
            err.to_string()
                .contains("legacy skill path is not a file, directory, or symlink"),
            "unexpected cleanup error: {err}"
        );
        assert!(
            legacy_path.exists(),
            "unsupported legacy path should be left untouched"
        );
    }
}
