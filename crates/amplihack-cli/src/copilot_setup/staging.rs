//! Asset staging — agents, skills, command docs, and plugin registration.

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::{copilot_home, fs_helpers, staged_framework_dir};

pub(super) fn stage_agents(source_agents: &Path, copilot_home: &Path) -> Result<usize> {
    let dest = copilot_home.join("agents").join("amplihack");
    fs_helpers::reset_markdown_dir(&dest)?;
    fs_helpers::flatten_markdown_tree(source_agents, &dest)
}

pub(super) fn stage_directory(
    source_dir: &Path,
    copilot_home: &Path,
    dest_name: &str,
) -> Result<usize> {
    let dest = copilot_home.join(dest_name).join("amplihack");
    fs_helpers::reset_markdown_dir(&dest)?;
    fs_helpers::flatten_markdown_tree(source_dir, &dest)
}

pub(super) fn stage_skills(source_skills: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_skills.is_dir() {
        return Ok(0);
    }

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
    let mut config: Value = if config_path.is_file() {
        serde_json::from_str(&fs::read_to_string(&config_path)?)?
    } else {
        json!({})
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

    fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    Ok(true)
}
