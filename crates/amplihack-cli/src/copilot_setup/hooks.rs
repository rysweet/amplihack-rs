//! Hook wiring, instructions generation, and related helpers.

use anyhow::{Context, Result};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{
    COPILOT_HOOK_WRAPPERS, COPILOT_HOOKS_MANIFEST, HookWrapperSpec, INSTRUCTIONS_MARKER_END,
    INSTRUCTIONS_MARKER_START, copilot_home, fs_helpers,
};

pub(super) fn stage_repo_hooks(repo_root: &Path) -> Result<usize> {
    let hooks_dir = repo_root.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    let manifest_dest = hooks_dir.join("amplihack-hooks.json");
    fs::write(&manifest_dest, COPILOT_HOOKS_MANIFEST)?;

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
    if let Some(start) = existing.find(INSTRUCTIONS_MARKER_START) {
        if let Some(end) = existing.find(INSTRUCTIONS_MARKER_END) {
            let before = &existing[..start];
            let after = &existing[end + INSTRUCTIONS_MARKER_END.len()..];
            return format!("{before}{section}{after}");
        }
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
