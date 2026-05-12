//! Register amplihack as a GitHub Copilot CLI plugin so the `amplihack-hooks`
//! native binary fires for Copilot CLI sessions, not just Claude Code.
//!
//! The Copilot CLI plugin spec
//! (<https://docs.github.com/copilot/concepts/agents/copilot-cli/about-cli-plugins>,
//! <https://docs.github.com/en/copilot/reference/cli-plugin-reference>) loads
//! hooks from a `hooks.json` declared in the plugin's `plugin.json` (default
//! resolution path is `<plugin>/hooks.json`). Pre-fix the rust install only
//! wrote `~/.claude/settings.json` for Claude Code, so Copilot CLI sessions
//! got zero hook coverage (no SessionStart, PreToolUse, PostToolUse,
//! UserPromptSubmit, Stop) — see issue #577.
//!
//! This module:
//! 1. Stages `~/.copilot/installed-plugins/amplihack@local/` with a
//!    `plugin.json` that declares `hooks: "./hooks.json"` (and `commands: "./commands"`
//!    when commands ship in the source tree).
//! 2. Writes `hooks.json` with one entry per Copilot CLI hook event,
//!    invoking the `amplihack-hooks` binary with the matching subcommand.
//! 3. Idempotently registers the plugin in `~/.copilot/config.json` under
//!    `installedPlugins`.
//!
//! When `~/.copilot/` does not exist (Copilot CLI not installed on this
//! host), the module is a no-op and returns `Ok(false)`.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use crate::copilot_setup::jsonc as jsonc_utils;

use super::{
    binary::{validate_binary_path, validate_hook_command_string},
    hooks::shell_quote_path,
    paths::home_dir,
};

/// Public entry: register amplihack as a local Copilot CLI plugin.
///
/// `repo_root` is the source repository the install is reading from (used
/// to locate the slash-command markdown files for inclusion in the plugin).
/// `hooks_bin` is the path to the deployed `amplihack-hooks` binary that
/// the generated `hooks.json` should invoke.
///
/// Returns `Ok(true)` if the plugin was created/refreshed, `Ok(false)` if
/// Copilot CLI isn't present on this host (no `~/.copilot/`).
pub(super) fn register_copilot_plugin(repo_root: &Path, hooks_bin: &Path) -> Result<bool> {
    register_copilot_plugin_in(&home_dir()?.join(".copilot"), repo_root, hooks_bin)
}

/// Test-friendly variant: registers the plugin under an explicit `copilot_home`
/// directory instead of resolving `~/.copilot` from the environment. This avoids
/// mutating the global `HOME` env var inside parallel unit tests.
pub(super) fn register_copilot_plugin_in(
    copilot_home: &Path,
    repo_root: &Path,
    hooks_bin: &Path,
) -> Result<bool> {
    if !copilot_home.exists() {
        // Copilot CLI not installed — nothing to do.
        return Ok(false);
    }
    validate_copilot_hooks_bin(hooks_bin)?;

    let plugin_dir = copilot_home
        .join("installed-plugins")
        .join("amplihack@local");

    fs::create_dir_all(&plugin_dir)
        .with_context(|| format!("failed to create {}", plugin_dir.display()))?;

    // Stage commands first (if present) so plugin.json can advertise them.
    let commands_staged = stage_commands(repo_root, &plugin_dir)?;

    write_plugin_manifest(&plugin_dir, commands_staged)?;
    write_hooks_json(&plugin_dir, hooks_bin)?;
    register_in_config(copilot_home, &plugin_dir)?;

    Ok(true)
}

/// Write the plugin manifest. Always declares hooks; declares commands
/// only when at least one command markdown file was staged.
fn write_plugin_manifest(plugin_dir: &Path, commands_staged: bool) -> Result<()> {
    let mut manifest = json!({
        "name": "amplihack",
        "description": "amplihack framework — structured agentic development workflows, hooks, and commands",
        "version": crate::VERSION,
        "author": { "name": "amplihack" },
        "license": "MIT",
        "hooks": "./hooks.json",
    });
    if commands_staged {
        manifest
            .as_object_mut()
            .expect("manifest is a json object literal")
            .insert("commands".to_string(), json!("./commands"));
    }
    let manifest_path = plugin_dir.join("plugin.json");
    let body = serde_json::to_string_pretty(&manifest)
        .context("failed to serialize amplihack@local plugin.json")?;
    atomic_write(&manifest_path, body.as_bytes())?;
    Ok(())
}

/// Generate the Copilot CLI `hooks.json` referencing the rust hooks binary.
///
/// Event-name mapping (Claude Code → Copilot CLI):
/// - SessionStart      → sessionStart
/// - Stop              → sessionEnd          (closest analog)
/// - UserPromptSubmit  → userPromptSubmitted
/// - PreToolUse        → preToolUse
/// - PostToolUse       → postToolUse
///
/// `PreCompact` has no Copilot CLI analog (Copilot's `/compact` command
/// doesn't fire a hook event in the documented spec) so it's omitted.
fn write_hooks_json(plugin_dir: &Path, hooks_bin: &Path) -> Result<()> {
    let bin = validate_copilot_hooks_bin(hooks_bin)?;
    let session_start = copilot_hook_command(&bin, "session-start")?;
    let session_end = copilot_hook_command(&bin, "stop")?;
    let workflow_classification = copilot_hook_command(&bin, "workflow-classification-reminder")?;
    let user_prompt_submit = copilot_hook_command(&bin, "user-prompt-submit")?;
    let pre_tool_use = copilot_hook_command(&bin, "pre-tool-use")?;
    let post_tool_use = copilot_hook_command(&bin, "post-tool-use")?;

    let body = json!({
        "version": 1,
        "hooks": {
            "sessionStart": [{
                "type": "command",
                "bash": session_start,
                "timeoutSec": 10
            }],
            "sessionEnd": [{
                "type": "command",
                "bash": session_end,
                "timeoutSec": 120
            }],
            "userPromptSubmitted": [
                {
                    "type": "command",
                    "bash": workflow_classification,
                    "timeoutSec": 5
                },
                {
                    "type": "command",
                    "bash": user_prompt_submit,
                    "timeoutSec": 10
                }
            ],
            "preToolUse": [{
                "type": "command",
                "bash": pre_tool_use,
                "timeoutSec": 30
            }],
            "postToolUse": [{
                "type": "command",
                "bash": post_tool_use,
                "timeoutSec": 30
            }]
        }
    });
    let path = plugin_dir.join("hooks.json");
    let pretty = serde_json::to_string_pretty(&body)
        .context("failed to serialize amplihack@local hooks.json")?;
    atomic_write(&path, pretty.as_bytes())?;
    println!(
        "  ✅ Copilot CLI plugin hooks.json written to {}",
        path.display()
    );
    Ok(())
}

fn validate_copilot_hooks_bin(hooks_bin: &Path) -> Result<String> {
    let bin = hooks_bin.display().to_string();
    validate_binary_path(&bin).with_context(|| {
        format!(
            "unsafe Copilot CLI hooks binary path: {}",
            hooks_bin.display()
        )
    })?;
    Ok(bin)
}

fn copilot_hook_command(bin: &str, subcmd: &str) -> Result<String> {
    let command = format!("{} {}", shell_quote_path(bin), subcmd);
    validate_hook_command_string(&command)
        .with_context(|| format!("unsafe Copilot CLI hook command for {subcmd}"))?;
    Ok(command)
}

/// Stage slash-command markdown files into the plugin's `commands/` dir.
///
/// In the bundle layout the canonical command markdowns live at
/// `<repo>/docs/claude/commands/amplihack/`; in legacy `.claude` checkouts
/// they live at `<repo>/.claude/commands/amplihack/` (or one parent up).
/// Both locations are probed; the first match wins.
///
/// Returns `Ok(true)` if at least one `*.md` was copied — used by
/// [`write_plugin_manifest`] to decide whether to advertise `commands`.
fn stage_commands(repo_root: &Path, plugin_dir: &Path) -> Result<bool> {
    let candidates = [
        repo_root
            .join("docs")
            .join("claude")
            .join("commands")
            .join("amplihack"),
        repo_root.join(".claude").join("commands").join("amplihack"),
        repo_root
            .parent()
            .map(|p| p.join(".claude").join("commands").join("amplihack"))
            .unwrap_or_default(),
    ];
    let source = candidates.iter().find(|p| p.is_dir());
    let Some(source) = source else {
        return Ok(false);
    };

    let target = plugin_dir.join("commands");
    let staging = staging_dir(&target);
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    fs::create_dir_all(&staging)
        .with_context(|| format!("failed to create {}", staging.display()))?;

    let mut copied = 0_usize;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(|e| e.to_str()) == Some("md")
        {
            let dst = staging.join(entry.file_name());
            fs::copy(entry.path(), &dst).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    dst.display()
                )
            })?;
            copied += 1;
        }
    }

    if copied == 0 {
        let _ = fs::remove_dir_all(&staging);
        return Ok(false);
    }

    if target.exists() {
        let backup = backup_dir(&target);
        if backup.exists() {
            let _ = fs::remove_dir_all(&backup);
        }
        fs::rename(&target, &backup).with_context(|| {
            format!(
                "failed to back up existing {} to {}",
                target.display(),
                backup.display()
            )
        })?;
        if let Err(err) = fs::rename(&staging, &target) {
            let _ = fs::rename(&backup, &target);
            let _ = fs::remove_dir_all(&staging);
            return Err(err)
                .with_context(|| format!("failed to swap commands into {}", target.display()));
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(&staging, &target)
            .with_context(|| format!("failed to move commands into {}", target.display()))?;
    }

    println!(
        "  ✅ Copilot CLI plugin staged {copied} command(s) at {}",
        target.display()
    );
    Ok(true)
}

/// Idempotently insert the amplihack@local entry into
/// `~/.copilot/config.json` under `installedPlugins`. Preserves any existing
/// leading JSONC header while tolerating inline and block comments in the JSON
/// body. Parse failures are returned to the installer so Copilot hook readiness
/// cannot silently degrade into a success-shaped install without the plugin.
fn register_in_config(copilot_home: &Path, plugin_dir: &Path) -> Result<()> {
    let config_path = copilot_home.join("config.json");
    let (mut config, prefix): (Value, String) = if config_path.exists() {
        let raw = fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let prefix = jsonc_utils::leading_comment_prefix(&raw).to_string();
        let stripped = jsonc_utils::strip_jsonc_comments(&raw);
        let value = if stripped.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&stripped)
                .with_context(|| format!("failed to parse {} as JSONC", config_path.display()))?
        };
        (value, prefix)
    } else {
        (json!({}), String::new())
    };

    let now = chrono::Utc::now().to_rfc3339();

    let entry = json!({
        "name": "amplihack",
        "marketplace": "local",
        "version": crate::VERSION,
        "enabled": true,
        "cache_path": plugin_dir.display().to_string(),
        "source": "local",
        "installed_at": now,
    });

    let obj = config
        .as_object_mut()
        .context("Copilot config.json root is not a JSON object")?;
    let plugins_key = "installedPlugins";
    let plugins = obj
        .entry(plugins_key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let arr = plugins
        .as_array_mut()
        .context("Copilot config.json `installedPlugins` is not an array")?;
    arr.retain(|p| p.get("name").and_then(|n| n.as_str()) != Some("amplihack"));
    arr.push(entry);

    let body =
        serde_json::to_string_pretty(&config).context("failed to serialize Copilot config.json")?;
    atomic_write(
        &config_path,
        jsonc_utils::apply_prefix(&prefix, body).as_bytes(),
    )?;
    Ok(())
}

/// Atomically write `body` to `path` via temp-file + rename so concurrent
/// readers never observe a torn write.
fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    fs::write(&tmp, body).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

fn staging_dir(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".staging");
    target.with_file_name(name)
}

fn backup_dir(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".old");
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fake_repo(td: &TempDir, with_commands: bool) -> PathBuf {
        let root = td.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        if with_commands {
            let cmd_dir = root
                .join("docs")
                .join("claude")
                .join("commands")
                .join("amplihack");
            fs::create_dir_all(&cmd_dir).unwrap();
            fs::write(cmd_dir.join("dev.md"), "# /dev\n").unwrap();
            fs::write(cmd_dir.join("analyze.md"), "# /analyze\n").unwrap();
            fs::write(cmd_dir.join("not-a-command.txt"), "ignored\n").unwrap();
        }
        root
    }

    fn fake_copilot_home(td: &TempDir) -> PathBuf {
        let h = td.path().join(".copilot");
        fs::create_dir_all(&h).unwrap();
        h
    }

    fn run_with_copilot_home(copilot_home: &Path, repo_root: &Path, hooks_bin: &Path) -> bool {
        register_copilot_plugin_in(copilot_home, repo_root, hooks_bin).expect("registration failed")
    }

    #[test]
    fn noop_when_copilot_home_missing() {
        let td = TempDir::new().unwrap();
        // Note: no .copilot dir created.
        let res = register_copilot_plugin_in(
            &td.path().join(".copilot"),
            &td.path().join("repo"),
            Path::new("/usr/bin/true"),
        )
        .expect("should succeed even without copilot");
        assert!(!res, "should return false when ~/.copilot is missing");
    }

    #[test]
    fn writes_hooks_json_with_amplihack_hooks_subcommands() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();

        let res = run_with_copilot_home(&copilot_home, &repo, &hooks_bin);
        assert!(res, "registration should succeed when ~/.copilot exists");

        let plugin_dir = copilot_home
            .join("installed-plugins")
            .join("amplihack@local");
        let hooks_path = plugin_dir.join("hooks.json");
        assert!(hooks_path.exists(), "hooks.json should be written");
        let body: Value = serde_json::from_str(&fs::read_to_string(&hooks_path).unwrap()).unwrap();
        let hooks = body.get("hooks").and_then(|h| h.as_object()).unwrap();
        for evt in [
            "sessionStart",
            "sessionEnd",
            "userPromptSubmitted",
            "preToolUse",
            "postToolUse",
        ] {
            assert!(hooks.contains_key(evt), "hooks.json missing event {evt}");
        }
        let raw = fs::read_to_string(&hooks_path).unwrap();
        assert!(
            raw.contains("amplihack-hooks"),
            "hooks.json should invoke the amplihack-hooks binary"
        );
    }

    #[test]
    fn rejects_shell_metacharacters_in_hooks_binary_path() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let repo = fake_repo(&td, false);
        let unsafe_dir = td.path().join("bad$HOME");
        fs::create_dir_all(&unsafe_dir).unwrap();
        let hooks_bin = unsafe_dir.join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();

        let result = register_copilot_plugin_in(&copilot_home, &repo, &hooks_bin);

        assert!(
            result.is_err(),
            "shell metacharacters in hooks binary paths must be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unsafe Copilot CLI hooks binary path")
                || err.contains("unsafe character"),
            "error should identify unsafe hook binary path, got: {err}"
        );
        assert!(
            !copilot_home
                .join("installed-plugins")
                .join("amplihack@local")
                .exists(),
            "invalid hooks path must fail before staging a partial plugin"
        );
    }

    #[test]
    fn writes_plugin_manifest_with_hooks_field() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);

        let manifest_path = copilot_home
            .join("installed-plugins")
            .join("amplihack@local")
            .join("plugin.json");
        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            manifest.get("name").and_then(|n| n.as_str()),
            Some("amplihack")
        );
        assert_eq!(
            manifest.get("hooks").and_then(|h| h.as_str()),
            Some("./hooks.json")
        );
        assert!(
            manifest.get("commands").is_none(),
            "no commands should be advertised when none staged"
        );
    }

    #[test]
    fn stages_commands_when_present_and_advertises_them() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let repo = fake_repo(&td, true);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);

        let plugin_dir = copilot_home
            .join("installed-plugins")
            .join("amplihack@local");
        let cmd_dir = plugin_dir.join("commands");
        assert!(cmd_dir.is_dir(), "commands dir should be staged");
        assert!(cmd_dir.join("dev.md").exists(), "dev.md should be staged");
        assert!(
            cmd_dir.join("analyze.md").exists(),
            "analyze.md should be staged"
        );
        assert!(
            !cmd_dir.join("not-a-command.txt").exists(),
            "non-md files must not be staged"
        );

        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(plugin_dir.join("plugin.json")).unwrap())
                .unwrap();
        assert_eq!(
            manifest.get("commands").and_then(|c| c.as_str()),
            Some("./commands"),
            "manifest must advertise commands when staged"
        );
    }

    #[test]
    fn idempotent_registration_does_not_duplicate_entries() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);

        let after = fs::read_to_string(copilot_home.join("config.json")).unwrap();
        let json_start = after.find('{').expect("config must contain JSON body");
        let cfg: Value = serde_json::from_str(&after[json_start..]).unwrap();
        let plugins = cfg
            .get("installedPlugins")
            .and_then(|p| p.as_array())
            .expect("installedPlugins should exist");
        let amplihack = plugins
            .iter()
            .filter(|p| p.get("name").and_then(|n| n.as_str()) == Some("amplihack"))
            .count();
        assert_eq!(
            amplihack, 1,
            "amplihack plugin should appear exactly once after two registrations"
        );
    }

    #[test]
    fn preserves_unrelated_config_entries() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let cfg_path = copilot_home.join("config.json");
        fs::write(
            &cfg_path,
            r#"// Copilot CLI managed file
{
  "lastLoggedInUser": {"login": "alice"},
  "trustedFolders": ["/tmp"]
}
"#,
        )
        .unwrap();

        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);

        let after = fs::read_to_string(copilot_home.join("config.json")).unwrap();
        let json_start = after.find('{').expect("config must contain JSON body");
        let cfg: Value = serde_json::from_str(&after[json_start..]).unwrap();
        assert_eq!(
            cfg.get("lastLoggedInUser")
                .and_then(|u| u.get("login"))
                .and_then(|l| l.as_str()),
            Some("alice"),
            "registration must preserve unrelated fields"
        );
        assert!(
            cfg.get("trustedFolders").is_some(),
            "trustedFolders must be preserved"
        );
        let plugins = cfg
            .get("installedPlugins")
            .and_then(|p| p.as_array())
            .unwrap();
        assert_eq!(plugins.len(), 1, "amplihack should be registered");
    }

    #[test]
    fn preserves_leading_jsonc_comments_when_registering_plugin() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let cfg_path = copilot_home.join("config.json");
        let header = "// Copilot CLI managed file\n// Preserve this header\n";
        fs::write(
            &cfg_path,
            format!("{header}{{\n  \"lastLoggedInUser\": {{\"login\": \"alice\"}}\n}}\n"),
        )
        .unwrap();

        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);

        let after = fs::read_to_string(&cfg_path).unwrap();
        assert!(
            after.starts_with(header),
            "Copilot config JSONC header comments must be preserved verbatim; got:\n{after}"
        );
        let json_start = after
            .find('{')
            .expect("config must still contain JSON body");
        let cfg: Value = serde_json::from_str(&after[json_start..]).unwrap();
        assert!(
            cfg.get("installedPlugins")
                .and_then(|p| p.as_array())
                .is_some_and(|plugins| plugins
                    .iter()
                    .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("amplihack"))),
            "registration must still add amplihack installedPlugins entry"
        );
    }

    #[test]
    fn supports_inline_and_block_jsonc_comments_when_registering_plugin() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let cfg_path = copilot_home.join("config.json");
        fs::write(
            &cfg_path,
            r#"{
  "lastLoggedInUser": {"login": "alice"}, // inline comment from Copilot/user tooling
  /* existing user-managed block comment */
  "trustedFolders": ["/tmp"]
}
"#,
        )
        .unwrap();

        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        run_with_copilot_home(&copilot_home, &repo, &hooks_bin);

        let after = fs::read_to_string(&cfg_path).unwrap();
        assert!(
            after.contains("\"installedPlugins\""),
            "JSONC comments must not prevent amplihack plugin registration; got:\n{after}"
        );
        assert!(
            after.contains("\"trustedFolders\""),
            "registration must preserve unrelated config keys while rewriting JSONC; got:\n{after}"
        );
    }

    #[test]
    fn malformed_copilot_config_is_registration_error() {
        let td = TempDir::new().unwrap();
        let copilot_home = fake_copilot_home(&td);
        let cfg_path = copilot_home.join("config.json");
        fs::write(&cfg_path, "{ malformed json\n").unwrap();

        let repo = fake_repo(&td, false);
        let hooks_bin = td.path().join("amplihack-hooks");
        fs::write(&hooks_bin, b"#!/bin/sh\nexit 0\n").unwrap();

        let result = register_copilot_plugin_in(&copilot_home, &repo, &hooks_bin);

        assert!(
            result.is_err(),
            "malformed Copilot config must be surfaced as an install-blocking error, not silently skipped"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("config.json") || err.contains("JSON"),
            "registration error should identify the Copilot config parse failure, got: {err}"
        );
    }

    #[test]
    fn strip_jsonc_handles_leading_line_comments() {
        let raw = "// header line\n  // indented\n{\"a\":1}\n";
        let out = jsonc_utils::strip_jsonc_comments(raw);
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.get("a").and_then(|v| v.as_i64()), Some(1));
    }
}
