//! Issue #2279 (C5, D4): the launch repair of amplihack's hooks block.
//!
//! Runs after the #1526 install-stamp gate in
//! `ensure_framework_installed_opts`, so an older binary never writes its
//! older block. Any drift is repaired, not only a file with no amplihack
//! hook at all.

use super::hook_block::{FileDrift, HookTarget, drift_file};
use super::paths::{self, global_settings_path, unix_timestamp};
use super::settings::{
    LIMIT_RESUME_MIN_CLAUDE, claude_too_old_for_limit_resume, ensure_settings_json_for,
};
use super::types::LIMIT_RESUME_EVENT;
use super::{binary, interactive, status_line_command};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Issue #2279 (D4): the settings file the launch repair keeps current:
/// `$CLAUDE_CONFIG_DIR/settings.json` when that variable is set, non-empty
/// and absolute, else `~/.claude/settings.json`. The second value says
/// whether the repair may create the file; it never creates one in a
/// `CLAUDE_CONFIG_DIR`.
pub(super) fn launch_settings_target() -> Result<(PathBuf, bool)> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Ok((dir.join("settings.json"), false)),
        _ => Ok((global_settings_path()?, true)),
    }
}

/// Issue #2279 (D4): repairs any drift of amplihack's hooks block in the
/// launch target, not only a file with no amplihack hook at all. A current
/// file is not touched; a file that is not JSON or may not be written gets
/// one warning on `notice` and no write.
pub(super) fn repair_settings_hooks<W: std::io::Write>(
    notice: &mut W,
    staging_dir: &Path,
    hooks_bin: &Path,
) -> Result<()> {
    let (settings_path, may_create) = launch_settings_target()?;
    let shown = paths::sanitize_path(&settings_path);
    let Some(probe_target) = HookTarget::new(hooks_bin.to_path_buf(), true) else {
        let _ = writeln!(
            notice,
            "amplihack: warning: the hooks binary path {} holds a shell-unsafe character; \
             hook registrations were not checked or repaired.",
            hooks_bin.display()
        );
        return Ok(());
    };
    let with = |limit_resume| HookTarget {
        limit_resume,
        ..probe_target.clone()
    };
    // #1659: only a Claude launch asks `claude --version`. Another tool's
    // launch leaves a file that is current for either answer alone.
    if !crate::freshness::claude_selected()
        && (drift_file(&settings_path, &with(true)) == FileDrift::Clean
            || drift_file(&settings_path, &with(false)) == FileDrift::Clean)
    {
        return Ok(());
    }
    let old_claude = claude_too_old_for_limit_resume();
    let target = with(old_claude.is_none());
    match drift_file(&settings_path, &target) {
        FileDrift::Clean => return Ok(()),
        FileDrift::Absent if !may_create => return Ok(()),
        FileDrift::Absent => {}
        FileDrift::Drift(drift) => {
            if let Some(version) = &old_claude
                && drift.extra.iter().any(|e| e == LIMIT_RESUME_EVENT)
            {
                println!(
                    "⚠️  Claude Code {version} is older than {LIMIT_RESUME_MIN_CLAUDE}: removing \
                     the usage-limit resume hook (StopFailure) from settings.json, which this \
                     Claude Code would otherwise read as invalid and run no amplihack hook"
                );
            }
            tracing::warn!(
                "settings.json hooks drift ({}) — auto-repairing",
                drift.summary()
            );
        }
        FileDrift::Unparseable(reason) | FileDrift::Refused(reason) => {
            // Issue #1526 (SR-8): never back up or rewrite a file we cannot read.
            let _ = writeln!(
                notice,
                "amplihack: warning: {shown} could not be used ({reason}); hook registrations \
                 were not checked or repaired. Fix the file, then run `amplihack install`."
            );
            return Ok(());
        }
    }
    // The launch repair writes the user's own settings, so it is global.
    let status_line =
        status_line_command(interactive::HookScope::Global, binary::stable_self_binary);
    let (settings_ok, _events) = ensure_settings_json_for(
        &settings_path,
        staging_dir,
        unix_timestamp(),
        hooks_bin,
        status_line.as_deref(),
        target.limit_resume,
    )?;
    if !settings_ok || drift_file(&settings_path, &target) != FileDrift::Clean {
        bail!(
            "hooks still not registered in {shown} after auto-repair.\n\
             Run `amplihack install` manually to fix hook wiring, or `amplihack doctor` to \
             diagnose."
        );
    }
    println!("✅ Auto-repaired hook registrations in settings.json");
    Ok(())
}
