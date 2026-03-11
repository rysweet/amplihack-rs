//! Copilot home staging to match Python launcher behavior.

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const INSTRUCTIONS_MARKER_START: &str = "<!-- AMPLIHACK_INSTRUCTIONS_START -->";
const INSTRUCTIONS_MARKER_END: &str = "<!-- AMPLIHACK_INSTRUCTIONS_END -->";
const COPILOT_HOOKS_MANIFEST: &str = r#"{
  "version": 1,
  "hooks": {
    "sessionStart": [{"type": "command", "bash": ".github/hooks/session-start", "timeoutSec": 30}],
    "sessionEnd": [{"type": "command", "bash": ".github/hooks/session-stop", "timeoutSec": 30}],
    "userPromptSubmitted": [{"type": "command", "bash": ".github/hooks/user-prompt-submit", "timeoutSec": 10}],
    "preToolUse": [{"type": "command", "bash": ".github/hooks/pre-tool-use", "timeoutSec": 15}],
    "postToolUse": [{"type": "command", "bash": ".github/hooks/post-tool-use", "timeoutSec": 10}],
    "errorOccurred": [{"type": "command", "bash": ".github/hooks/error-occurred", "timeoutSec": 10}]
  }
}
"#;

struct HookWrapperSpec {
    hook_name: &'static str,
    python_files: &'static [&'static str],
}

const COPILOT_HOOK_WRAPPERS: &[HookWrapperSpec] = &[
    HookWrapperSpec {
        hook_name: "session-start",
        python_files: &["session_start.py"],
    },
    HookWrapperSpec {
        hook_name: "session-stop",
        python_files: &["stop.py", "session_stop.py"],
    },
    HookWrapperSpec {
        hook_name: "pre-tool-use",
        python_files: &["pre_tool_use.py"],
    },
    HookWrapperSpec {
        hook_name: "post-tool-use",
        python_files: &["post_tool_use.py"],
    },
    HookWrapperSpec {
        hook_name: "user-prompt-submit",
        python_files: &[
            "user_prompt_submit.py",
            "workflow_classification_reminder.py",
        ],
    },
];

pub fn ensure_copilot_home_staged() -> Result<()> {
    let source_root = staged_framework_dir()?;
    let copilot_home = copilot_home()?;
    fs::create_dir_all(&copilot_home)
        .with_context(|| format!("failed to create {}", copilot_home.display()))?;

    stage_agents(&source_root.join("agents").join("amplihack"), &copilot_home)?;
    stage_skills(&source_root.join("skills"), &copilot_home)?;
    stage_directory(&source_root.join("workflow"), &copilot_home, "workflow")?;
    stage_directory(&source_root.join("context"), &copilot_home, "context")?;
    stage_command_docs(
        &source_root.join("commands").join("amplihack"),
        &copilot_home,
    )?;
    register_plugin(
        &source_root.join("commands").join("amplihack"),
        &copilot_home,
    )?;
    stage_repo_hooks(
        &std::env::current_dir().context("failed to determine current working directory")?,
    )?;
    generate_copilot_instructions(&copilot_home)?;
    Ok(())
}

fn stage_agents(source_agents: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_agents.exists() {
        return Ok(0);
    }

    let dest = copilot_home.join("agents").join("amplihack");
    reset_markdown_dir(&dest)?;
    flatten_markdown_tree(source_agents, &dest)
}

fn stage_directory(source_dir: &Path, copilot_home: &Path, dest_name: &str) -> Result<usize> {
    if !source_dir.exists() {
        return Ok(0);
    }

    let dest = copilot_home.join(dest_name).join("amplihack");
    reset_markdown_dir(&dest)?;
    flatten_markdown_tree(source_dir, &dest)
}

fn stage_skills(source_skills: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_skills.exists() {
        return Ok(0);
    }

    let skills_dest = copilot_home.join("skills");
    fs::create_dir_all(&skills_dest)
        .with_context(|| format!("failed to create {}", skills_dest.display()))?;

    let mut copied = 0usize;
    for entry in fs::read_dir(source_skills)
        .with_context(|| format!("failed to read {}", source_skills.display()))?
    {
        let entry = entry?;
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let dest_skill = skills_dest.join(entry.file_name());
        let is_new = !dest_skill.exists();
        copy_dir_recursive(&skill_dir, &dest_skill)?;
        if is_new {
            copied += 1;
        }
    }
    Ok(copied)
}

fn stage_command_docs(source_commands: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_commands.exists() {
        return Ok(0);
    }

    let dest = copilot_home.join("commands").join("amplihack");
    fs::create_dir_all(&dest).with_context(|| format!("failed to create {}", dest.display()))?;

    let mut copied = 0usize;
    for entry in fs::read_dir(source_commands)
        .with_context(|| format!("failed to read {}", source_commands.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        fs::copy(&path, dest.join(entry.file_name())).with_context(|| {
            format!("failed to copy {} into {}", path.display(), dest.display())
        })?;
        copied += 1;
    }

    Ok(copied)
}

fn register_plugin(source_commands: &Path, copilot_home: &Path) -> Result<bool> {
    if !source_commands.exists() {
        return Ok(false);
    }

    let plugin_cache = copilot_home
        .join("installed-plugins")
        .join("amplihack@local");
    let plugin_commands = plugin_cache.join("commands");
    fs::create_dir_all(&plugin_commands)
        .with_context(|| format!("failed to create {}", plugin_commands.display()))?;

    if source_commands.join("plugin.json").exists() {
        fs::copy(
            source_commands.join("plugin.json"),
            plugin_cache.join("plugin.json"),
        )
        .with_context(|| format!("failed to copy plugin.json into {}", plugin_cache.display()))?;
    }

    let mut copied = 0usize;
    for entry in fs::read_dir(source_commands)
        .with_context(|| format!("failed to read {}", source_commands.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        fs::copy(&path, plugin_commands.join(entry.file_name())).with_context(|| {
            format!(
                "failed to copy {} into {}",
                path.display(),
                plugin_commands.display()
            )
        })?;
        copied += 1;
    }

    if copied == 0 {
        return Ok(false);
    }

    let config_path = copilot_home.join("config.json");
    let mut config = if config_path.exists() {
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    let Some(root) = config.as_object_mut() else {
        return Err(anyhow!("copilot config root must be a JSON object"));
    };
    let installed = root
        .entry("installed_plugins")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !installed.is_array() {
        *installed = Value::Array(Vec::new());
    }
    let installed = installed
        .as_array_mut()
        .expect("installed_plugins converted to array");
    installed.retain(|entry| entry.get("name").and_then(Value::as_str) != Some("amplihack"));
    installed.push(json!({
        "name": "amplihack",
        "marketplace": "local",
        "version": "1.0.0",
        "enabled": true,
        "cache_path": plugin_cache.to_string_lossy(),
        "source": "local"
    }));

    fs::write(&config_path, serde_json::to_string_pretty(&config)? + "\n")
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(true)
}

fn stage_repo_hooks(repo_root: &Path) -> Result<usize> {
    let hooks_dir = repo_root.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;
    let manifest_path = hooks_dir.join("amplihack-hooks.json");
    fs::write(&manifest_path, COPILOT_HOOKS_MANIFEST)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    let mut staged = 0usize;
    for spec in COPILOT_HOOK_WRAPPERS {
        let wrapper_path = hooks_dir.join(spec.hook_name);
        if should_preserve_user_hook(&wrapper_path)? {
            continue;
        }
        fs::write(&wrapper_path, build_wrapper_script(spec))
            .with_context(|| format!("failed to write {}", wrapper_path.display()))?;
        set_executable(&wrapper_path)?;
        staged += 1;
    }

    let error_wrapper = hooks_dir.join("error-occurred");
    if !should_preserve_user_hook(&error_wrapper)? {
        fs::write(&error_wrapper, error_wrapper_script())
            .with_context(|| format!("failed to write {}", error_wrapper.display()))?;
        set_executable(&error_wrapper)?;
        staged += 1;
    }

    Ok(staged)
}

fn generate_copilot_instructions(copilot_home: &Path) -> Result<()> {
    let instructions_path = copilot_home.join("copilot-instructions.md");
    let section = format!(
        "{INSTRUCTIONS_MARKER_START}\n# Amplihack Framework Integration\n\n\
You have access to the amplihack agentic coding framework. Use these resources:\n\n\
## Workflows\n\
Read workflow files from `{workflow}` to follow structured processes.\n\n\
## Context\n\
Read context files from `{context}` for project philosophy and patterns.\n\n\
## Commands\n\
Read command definitions from `{commands}` for available capabilities.\n\n\
## Agents\n\
Custom agents are available at `{agents}`.\n\n\
## Skills\n\
Skills are available at `{skills}`.\n{INSTRUCTIONS_MARKER_END}",
        workflow = copilot_home.join("workflow").join("amplihack").display(),
        context = copilot_home.join("context").join("amplihack").display(),
        commands = copilot_home.join("commands").join("amplihack").display(),
        agents = copilot_home.join("agents").join("amplihack").display(),
        skills = copilot_home.join("skills").display(),
    );

    let updated = if instructions_path.exists() {
        let existing = fs::read_to_string(&instructions_path)
            .with_context(|| format!("failed to read {}", instructions_path.display()))?;
        replace_or_append_section(&existing, &section)
    } else {
        format!("{section}\n")
    };

    fs::write(&instructions_path, updated)
        .with_context(|| format!("failed to write {}", instructions_path.display()))
}

fn replace_or_append_section(existing: &str, section: &str) -> String {
    if let Some(start) = existing.find(INSTRUCTIONS_MARKER_START)
        && let Some(end_rel) = existing[start..].find(INSTRUCTIONS_MARKER_END)
    {
        let end = start + end_rel + INSTRUCTIONS_MARKER_END.len();
        let mut updated = String::new();
        updated.push_str(&existing[..start]);
        updated.push_str(section);
        updated.push_str(&existing[end..]);
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        return updated;
    }

    if existing.trim().is_empty() {
        format!("{section}\n")
    } else {
        format!("{}\n\n{section}\n", existing.trim_end())
    }
}

fn reset_markdown_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn flatten_markdown_tree(source: &Path, dest: &Path) -> Result<usize> {
    let mut copied = 0usize;
    for path in walk_files(source)? {
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let file_name = path
            .file_name()
            .context("source markdown file missing name")?;
        fs::copy(&path, dest.join(file_name)).with_context(|| {
            format!("failed to copy {} into {}", path.display(), dest.display())
        })?;
        copied += 1;
    }
    Ok(copied)
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            paths.extend(walk_files(&path)?);
        } else {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target).with_context(|| {
                format!(
                    "failed to copy {} into {}",
                    path.display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

fn should_preserve_user_hook(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let existing =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(!existing.contains("amplihack"))
}

fn build_wrapper_script(spec: &HookWrapperSpec) -> String {
    if spec.python_files.len() == 1 {
        let hook = spec.python_files[0];
        return format!(
            "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack\n\
HOOK=\"{hook}\"\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\n\
if [[ -f \"${{AMPLIHACK_HOOKS}}/${{HOOK}}\" ]]; then\n\
    exec python3 \"${{AMPLIHACK_HOOKS}}/${{HOOK}}\" \"$@\"\n\
elif REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" && [[ -f \"${{REPO_ROOT}}/.claude/tools/amplihack/hooks/${{HOOK}}\" ]]; then\n\
    exec python3 \"${{REPO_ROOT}}/.claude/tools/amplihack/hooks/${{HOOK}}\" \"$@\"\n\
else\n\
    echo \"{{}}\"\n\
fi\n"
        );
    }

    let script_blocks = spec
        .python_files
        .iter()
        .map(|hook| {
            format!(
                "if [[ -f \"${{AMPLIHACK_HOOKS}}/{hook}\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${{AMPLIHACK_HOOKS}}/{hook}\" \"$@\" 2>/dev/null || true\n\
elif [[ -n \"$REPO_ROOT\" ]] && [[ -f \"${{REPO_ROOT}}/.claude/tools/amplihack/hooks/{hook}\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${{REPO_ROOT}}/.claude/tools/amplihack/hooks/{hook}\" \"$@\" 2>/dev/null || true\n\
fi"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack\n\
# Runs multiple hook scripts for this event\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\
REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" || REPO_ROOT=\"\"\n\
INPUT=$(cat)\n\n\
{script_blocks}\n"
    )
}

fn error_wrapper_script() -> &'static str {
    "#!/usr/bin/env bash\n\
# Copilot hook: error-occurred\n\
# Generated by amplihack\n\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\
LOG_DIR=\"$HOME/.amplihack/.claude/runtime/logs\"\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/error_occurred.py\" ]]; then\n\
    python3 \"${AMPLIHACK_HOOKS}/error_occurred.py\" \"$@\"\n\
elif REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/error_occurred.py\" ]]; then\n\
    python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/error_occurred.py\" \"$@\"\n\
else\n\
    mkdir -p \"$LOG_DIR\"\n\
    INPUT=$(cat)\n\
    ERROR_MSG=$(echo \"$INPUT\" | python3 -c \"import sys,json; print(json.load(sys.stdin).get('error',{}).get('message','unknown'))\" 2>/dev/null || echo \"unknown\")\n\
    echo \"$(date -Iseconds): ERROR - $ERROR_MSG\" >> \"${LOG_DIR}/errors.log\"\n\
    echo \"{}\"\n\
fi\n"
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let metadata =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

fn staged_framework_dir() -> Result<PathBuf> {
    home_dir().map(|home| home.join(".amplihack").join(".claude"))
}

fn copilot_home() -> Result<PathBuf> {
    home_dir().map(|home| home.join(".copilot"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("HOME is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_copilot_home_stages_assets_and_plugin() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = crate::test_support::cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let previous_cwd = crate::test_support::set_cwd(&repo_root).unwrap();

        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("agents/amplihack/core")).unwrap();
        fs::create_dir_all(staged.join("skills/dev-orchestrator")).unwrap();
        fs::create_dir_all(staged.join("skills/quality-audit")).unwrap();
        fs::create_dir_all(staged.join("workflow")).unwrap();
        fs::create_dir_all(staged.join("context")).unwrap();
        fs::create_dir_all(staged.join("commands/amplihack")).unwrap();
        fs::write(staged.join("agents/amplihack/core/architect.md"), "agent").unwrap();
        fs::write(staged.join("skills/dev-orchestrator/SKILL.md"), "skill-a").unwrap();
        fs::write(staged.join("skills/quality-audit/SKILL.md"), "skill-b").unwrap();
        fs::write(staged.join("workflow/DEFAULT_WORKFLOW.md"), "workflow").unwrap();
        fs::write(staged.join("context/USER_PREFERENCES.md"), "prefs").unwrap();
        fs::write(staged.join("commands/amplihack/dev.md"), "command").unwrap();
        fs::write(
            staged.join("commands/amplihack/plugin.json"),
            "{\"name\":\"amplihack\"}",
        )
        .unwrap();

        ensure_copilot_home_staged().unwrap();

        assert!(
            temp.path()
                .join(".copilot/agents/amplihack/architect.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/skills/dev-orchestrator/SKILL.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/skills/quality-audit/SKILL.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/workflow/amplihack/DEFAULT_WORKFLOW.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/commands/amplihack/dev.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/installed-plugins/amplihack@local/commands/dev.md")
                .exists()
        );
        assert!(repo_root.join(".github/hooks/session-start").exists());
        assert!(
            repo_root
                .join(".github/hooks/amplihack-hooks.json")
                .exists()
        );

        let config = fs::read_to_string(temp.path().join(".copilot/config.json")).unwrap();
        assert!(config.contains("\"name\": \"amplihack\""));

        crate::test_support::restore_cwd(&previous_cwd).unwrap();
        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn replace_or_append_section_updates_existing_block() {
        let existing =
            format!("before\n{INSTRUCTIONS_MARKER_START}\nold\n{INSTRUCTIONS_MARKER_END}\nafter\n");
        let updated = replace_or_append_section(&existing, "NEW");
        assert!(updated.contains("before"));
        assert!(updated.contains("after"));
        assert!(updated.contains("NEW"));
        assert!(!updated.contains("old"));
    }
}
