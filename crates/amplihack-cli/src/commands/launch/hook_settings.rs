//! Issue #2279 (C9, D5): `amplihack claude` passes amplihack's hooks block
//! as a settings source of its own, `--settings <AMPLIHACK_HOME>/runtime/
//! launch-hooks-settings.json`, so a stripped `~/.claude/settings.json`
//! cannot leave a session without amplihack's hooks.
//!
//! The file holds only `{"hooks": block}` (no `env`, no other key), is
//! written at mode 0600 in a 0700 `runtime/` directory only when its bytes
//! change, and is passed by path, never as inline JSON (#2030). Claude Code
//! runs identical hook commands from several settings sources once, and the
//! home file and this file are both built by `hook_block::block`, so their
//! commands are byte-identical.

use crate::commands::install::hook_block::{HookTarget, block, hook_target, reason};
use std::path::{Path, PathBuf};

/// The launch file, relative to the amplihack home.
pub(crate) const LAUNCH_FILE: &str = "runtime/launch-hooks-settings.json";

/// The launch file of this process's amplihack home, when that home is
/// known and absolute (SR-11). Doctor reads it; nothing here creates it.
pub(crate) fn launch_file_path() -> Option<PathBuf> {
    super::contract_file::contract_home()
        .ok()
        .filter(|home| home.is_absolute())
        .map(|home| home.join(LAUNCH_FILE))
}

/// Whether and what to pass as `--settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Inject {
    Path(PathBuf),
    /// The amplihack home or `runtime/` is not private (SR-15). The one skip
    /// the user can fix, so the launch warns about it.
    NotPrivate,
    Skip(&'static str),
}

/// Everything [`prepare_with`] decides on.
pub(super) struct Inputs<'a> {
    pub binary_name: &'a str,
    pub extra_args: &'a [String],
    /// The LiteLLM proxy (safe mode) is in use; it passes
    /// `--setting-sources ""` and keeps its own behaviour.
    pub proxy: bool,
    /// Implicit publication is refused by origin, opt-out or version checks.
    pub newer_stamp: bool,
    pub target: Option<&'a HookTarget>,
    pub amplihack_home: &'a Path,
}

/// Whether the user passed `--settings` (either form) before a literal `--`.
pub(super) fn user_passed_settings(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--settings" || arg.starts_with("--settings="))
}

/// The production [`prepare_with`], with its inputs read from this process.
pub(super) fn prepare(binary_name: &str, extra_args: &[String]) -> Inject {
    if binary_name != "claude" {
        return Inject::Skip("not claude");
    }
    let Ok(amplihack_home) = super::contract_file::contract_home() else {
        return Inject::Skip("no amplihack home");
    };
    if !amplihack_home.is_absolute() {
        return Inject::Skip("amplihack home is not absolute");
    }
    let proxy = amplihack_utils::litellm_proxy::proxy_requested();
    // Origin, the opt-out and the version stamp all govern publication.
    let newer_stamp = !crate::commands::install::binary_is_current();
    let target = if newer_stamp || proxy || user_passed_settings(extra_args) {
        None
    } else {
        hook_target()
    };
    prepare_with(&Inputs {
        binary_name,
        extra_args,
        proxy,
        newer_stamp,
        target: target.as_ref(),
        amplihack_home: &amplihack_home,
    })
}

/// Writes the launch file when it applies and returns its path, or why no
/// `--settings` is passed. A failed write logs one warning; the launch goes
/// on without the flag.
pub(super) fn prepare_with(i: &Inputs<'_>) -> Inject {
    if i.binary_name != "claude" {
        return Inject::Skip("not claude");
    }
    if i.proxy {
        return Inject::Skip("LiteLLM proxy in use");
    }
    if user_passed_settings(i.extra_args) {
        eprintln!(
            "amplihack: you passed --settings, so amplihack does not add its hooks settings \
             file; ~/.claude/settings.json still holds the hooks."
        );
        return Inject::Skip("user passed --settings");
    }
    if i.newer_stamp {
        return Inject::Skip("executable is not eligible for implicit hook publication");
    }
    let Some(target) = i.target else {
        return Inject::Skip("no hooks binary");
    };
    if !home_is_private(i.amplihack_home) {
        return Inject::NotPrivate;
    }
    let path = i.amplihack_home.join(LAUNCH_FILE);
    match write_if_changed(&path, target) {
        Ok(()) => Inject::Path(path),
        Err(WriteError::NotPrivate) => Inject::NotPrivate,
        Err(WriteError::Io(reason)) => {
            tracing::warn!("launch hooks settings file not written: {reason}");
            eprintln!(
                "amplihack: warning: {} not written ({reason}); launching without it.",
                path.display()
            );
            Inject::Skip("write failed")
        }
    }
}

enum WriteError {
    NotPrivate,
    Io(String),
}

/// The amplihack home must be a real directory owned by this user that no
/// group or other user can write (SR-15).
#[cfg(unix)]
fn home_is_private(home: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(home).is_ok_and(|m| {
        m.is_dir() && m.uid() == amplihack_utils::secure_files::euid() && m.mode() & 0o022 == 0
    })
}

#[cfg(not(unix))]
fn home_is_private(_home: &Path) -> bool {
    false
}

/// Creates or checks `runtime/` (0700, ours, not a symlink), tightening a
/// directory of ours that an older install left at a wider mode.
#[cfg(unix)]
fn private_runtime_dir(dir: &Path) -> Result<(), WriteError> {
    use amplihack_utils::secure_files::{ensure_private_dir, euid};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if ensure_private_dir(dir).is_ok() {
        return Ok(());
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|_| WriteError::NotPrivate)?;
    if meta.file_type().is_symlink() || !meta.is_dir() || meta.uid() != euid() {
        return Err(WriteError::NotPrivate);
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| WriteError::Io(format!("runtime dir mode not set: {}", e.kind())))?;
    tracing::info!("tightened {} to mode 0700", dir.display());
    ensure_private_dir(dir).map_err(|_| WriteError::NotPrivate)
}

#[cfg(unix)]
fn write_if_changed(path: &Path, target: &HookTarget) -> Result<(), WriteError> {
    use amplihack_utils::secure_files::{read_bounded_nofollow, write_private_atomic};
    let dir = path.parent().ok_or(WriteError::NotPrivate)?;
    private_runtime_dir(dir)?;
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({ "hooks": block(target) }))
        .map_err(|e| WriteError::Io(format!("serialise: {:?}", e.classify())))?;
    bytes.push(b'\n');
    let limit = bytes.len() as u64 + 1;
    if let Ok(Some((current, _))) = read_bounded_nofollow(path, limit)
        && current == bytes
    {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(WriteError::NotPrivate)?;
    write_private_atomic(dir, name, &bytes).map_err(|e| WriteError::Io(reason(&e)))
}

#[cfg(not(unix))]
fn write_if_changed(_path: &Path, _target: &HookTarget) -> Result<(), WriteError> {
    Err(WriteError::NotPrivate)
}
