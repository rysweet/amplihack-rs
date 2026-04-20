//! Hook wiring, instructions generation, and related helpers.

use anyhow::{Context, Result};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{
    COPILOT_HOOK_TIMEOUT_SEC, COPILOT_HOOK_WRAPPERS, HookWrapperSpec, INSTRUCTIONS_MARKER_END,
    INSTRUCTIONS_MARKER_START, fs_helpers,
};

/// Build the Copilot hooks manifest as JSON. Hook entries reference absolute
/// paths to the staged bash wrappers under `<repo>/.github/hooks/`.
pub(super) fn build_copilot_hooks_manifest(hooks_dir: &Path) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for spec in COPILOT_HOOK_WRAPPERS {
        let bash_path = hooks_dir.join(spec.hook_name);
        let entry = serde_json::json!({
            "type": "command",
            "bash": bash_path.to_string_lossy(),
            "timeoutSec": COPILOT_HOOK_TIMEOUT_SEC,
        });
        hooks.insert(spec.copilot_event.to_string(), serde_json::json!([entry]));
    }
    serde_json::json!({
        "version": 1,
        "hooks": hooks,
    })
}

pub(super) fn stage_repo_hooks(repo_root: &Path) -> Result<usize> {
    let hooks_dir = repo_root.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    let manifest = build_copilot_hooks_manifest(&hooks_dir);
    let manifest_dest = hooks_dir.join("amplihack-hooks.json");
    fs::write(
        &manifest_dest,
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;

    let mut count = 1; // manifest itself

    for spec in COPILOT_HOOK_WRAPPERS {
        let dest = hooks_dir.join(spec.hook_name);
        if should_preserve_user_hook(&dest)? {
            continue;
        }
        let script = build_wrapper_script(spec);
        fs::write(&dest, &script)?;
        set_executable(&dest)?;
        count += 1;
    }

    let error_dest = hooks_dir.join("_error_handler");
    fs::write(&error_dest, error_wrapper_script())?;
    set_executable(&error_dest)?;
    count += 1;

    Ok(count)
}

/// Merge an amplihack hooks block into `~/.copilot/config.json` so that hooks
/// fire regardless of which directory Copilot is launched from. Existing
/// non-amplihack hook entries are preserved; amplihack-owned entries (those
/// whose `bash` path resolves under `~/.amplihack` or whose `_amplihack`
/// marker is set) are replaced atomically.
pub(super) fn write_user_level_hooks(copilot_home: &Path) -> Result<()> {
    // We point the user-level hooks at the staged framework directory so the
    // hook wrappers work irrespective of which repo Copilot opens. The
    // wrappers themselves resolve `amplihack-hooks` from PATH or the standard
    // install locations, so they degrade gracefully if the binary is missing.
    let hooks_dir = copilot_home.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    // Materialize the wrappers under ~/.copilot/.github/hooks/ so their bash
    // paths resolve absolutely on disk.
    for spec in COPILOT_HOOK_WRAPPERS {
        let dest = hooks_dir.join(spec.hook_name);
        let script = build_wrapper_script(spec);
        fs::write(&dest, &script)?;
        set_executable(&dest)?;
    }

    let config_path = copilot_home.join("config.json");
    let mut root: serde_json::Value = if config_path.is_file() {
        let raw = fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?;
        if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&raw).with_context(|| {
                format!(
                    "parse {} as JSON before merging hooks",
                    config_path.display()
                )
            })?
        }
    } else {
        serde_json::json!({})
    };

    let manifest = build_copilot_hooks_manifest(&hooks_dir);
    let amplihack_hooks = manifest
        .get("hooks")
        .and_then(|h| h.as_object())
        .cloned()
        .unwrap_or_default();

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config.json root is not a JSON object"))?;
    let hooks_entry = obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let hooks_obj = hooks_entry
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config.json `hooks` is not a JSON object"))?;

    // Replace amplihack-owned entries; keep any user-defined non-amplihack
    // hooks the user has registered for the same event by appending after.
    for (event, new_arr_value) in amplihack_hooks {
        let new_arr = new_arr_value.as_array().cloned().unwrap_or_default();
        let preserved: Vec<serde_json::Value> = hooks_obj
            .get(&event)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| !is_amplihack_owned(entry))
            .collect();
        let mut merged = new_arr;
        merged.extend(preserved);
        hooks_obj.insert(event, serde_json::Value::Array(merged));
    }

    let serialized = serde_json::to_string_pretty(&root)? + "\n";
    fs::write(&config_path, serialized)
        .with_context(|| format!("write {}", config_path.display()))?;
    Ok(())
}

fn is_amplihack_owned(entry: &serde_json::Value) -> bool {
    let bash = entry
        .get("bash")
        .and_then(|v| v.as_str())
        .or_else(|| entry.get("command").and_then(|v| v.as_str()))
        .unwrap_or("");
    bash.contains(".amplihack")
        || bash.contains("/.copilot/.github/hooks/")
        || bash.contains("/.github/hooks/session-start")
        || bash.contains("/.github/hooks/user-prompt-submit")
        || bash.contains("/.github/hooks/pre-tool-use")
        || bash.contains("/.github/hooks/post-tool-use")
        || bash.contains("/.github/hooks/pre-compact")
        || bash.contains("/.github/hooks/stop")
}

pub(super) fn generate_copilot_instructions(copilot_home: &Path) -> Result<()> {
    let agents_dir = copilot_home.join("agents").join("amplihack");
    if !agents_dir.is_dir() {
        return Ok(());
    }

    let instructions_path = copilot_home.join("instructions.md");
    let existing = fs::read_to_string(&instructions_path).unwrap_or_default();

    let mut section = String::new();
    section.push_str(INSTRUCTIONS_MARKER_START);
    section.push('\n');

    let mut agent_files = fs_helpers::walk_files(&agents_dir)?;
    agent_files.sort();
    for file in agent_files {
        if file.extension().is_some_and(|ext| ext == "md") {
            let content = fs::read_to_string(&file)
                .with_context(|| format!("read agent instructions {}", file.display()))?;
            section.push_str(&content);
            section.push('\n');
        }
    }

    section.push_str(INSTRUCTIONS_MARKER_END);
    section.push('\n');

    let updated = replace_or_append_section(&existing, &section);
    fs::write(&instructions_path, updated)?;

    Ok(())
}

pub(super) fn replace_or_append_section(existing: &str, section: &str) -> String {
    if let Some(start) = existing.find(INSTRUCTIONS_MARKER_START)
        && let Some(end) = existing.find(INSTRUCTIONS_MARKER_END)
    {
        let before = &existing[..start];
        let after = &existing[end + INSTRUCTIONS_MARKER_END.len()..];
        return format!("{before}{section}{after}");
    }

    if existing.is_empty() {
        return section.to_string();
    }

    format!("{existing}\n{section}")
}

fn should_preserve_user_hook(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let content = fs::read_to_string(path).unwrap_or_default();
    Ok(!content.contains("amplihack") && !content.contains("AMPLIHACK"))
}

pub(super) fn build_wrapper_script(spec: &HookWrapperSpec) -> String {
    let mut script = String::from("#!/usr/bin/env bash\nset -euo pipefail\n\n");

    script.push_str("HOOKS_BIN=\"\"\n");
    script.push_str("for candidate in \\\n");
    script.push_str("    \"$(command -v amplihack-hooks 2>/dev/null)\" \\\n");
    script.push_str("    \"${HOME}/.amplihack/target/release/amplihack-hooks\" \\\n");
    script.push_str("    \"${HOME}/.amplihack/target/debug/amplihack-hooks\"; do\n");
    script.push_str("  if [ -n \"$candidate\" ] && [ -x \"$candidate\" ]; then\n");
    script.push_str("    HOOKS_BIN=\"$candidate\"\n");
    script.push_str("    break\n");
    script.push_str("  fi\n");
    script.push_str("done\n\n");

    script.push_str("if [ -z \"$HOOKS_BIN\" ]; then\n");
    script.push_str("  echo \"[amplihack] hooks binary not found, skipping\" >&2\n");
    script.push_str("  exit 0\n");
    script.push_str("fi\n\n");

    if spec.subcommands.len() == 1 {
        script.push_str(&format!(
            "exec \"$HOOKS_BIN\" {} \"$@\"\n",
            spec.subcommands[0]
        ));
    } else {
        for sub in spec.subcommands {
            script.push_str(&format!("\"$HOOKS_BIN\" {sub} \"$@\" || true\n"));
        }
    }

    script
}

pub(super) fn error_wrapper_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail
LOG="${HOME}/.amplihack/logs/errors.log"
[ -f "$LOG" ] || exit 0
COUNT=$(wc -l < "$LOG" 2>/dev/null || echo 0)
if [ "$COUNT" -gt 0 ]; then
  echo "[amplihack] $COUNT error(s) in $LOG"
  sed -n '1,5p' "$LOG"
fi
"#
}

pub(super) fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let perms = fs::metadata(path)?.permissions();
        let mut new_perms = perms;
        new_perms.set_mode(new_perms.mode() | 0o111);
        fs::set_permissions(path, new_perms)?;
    }
    let _ = path; // suppress unused warning on non-Unix
    Ok(())
}
