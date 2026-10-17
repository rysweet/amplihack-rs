//! `profile login` and `profile run`.

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use crate::binary_finder::BinaryInfo;
use crate::commands::launch;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::process::Command;

/// Builds the child command with the profile directory verified at call time.
/// Arguments are an argv vector; no shell is involved.
pub(super) fn build_profile_command(
    binary: &BinaryInfo,
    store: &ProfileStore,
    name: &ProfileName,
    args: &[String],
) -> Result<Command, ProfileError> {
    // Validate ownership and mode-bit permissions immediately before creating
    // the child process. `resolve` alone only checks path containment.
    let (mut cmd, dir) = pinned_command(binary, store, name, args)?;
    cmd.env("CLAUDE_CONFIG_DIR", &dir);
    Ok(cmd)
}

/// Like [`build_profile_command`], but for a profile whose login is live in
/// `~/.claude`: `CLAUDE_CONFIG_DIR` is removed (never set to `~/.claude`), so
/// `claude` reads `~/.claude` and `~/.claude.json`. The profile is still
/// pinned for descendants and its directory still checked.
pub(super) fn build_default_dir_command(
    binary: &BinaryInfo,
    store: &ProfileStore,
    name: &ProfileName,
    args: &[String],
) -> Result<Command, ProfileError> {
    let (mut cmd, _) = pinned_command(binary, store, name, args)?;
    cmd.env_remove("CLAUDE_CONFIG_DIR");
    Ok(cmd)
}

fn pinned_command(
    binary: &BinaryInfo,
    store: &ProfileStore,
    name: &ProfileName,
    args: &[String],
) -> Result<(Command, std::path::PathBuf), ProfileError> {
    let dir = store.verify_profile_dir(name)?;
    // `pin_profile_for_descendants` reads the name and root from the path.
    debug_assert_eq!(dir.file_name(), Some(OsStr::new(name.as_str())));
    let mut cmd = Command::new(&binary.path);
    cmd.args(args);
    // Amplihack launches started inside this session keep its profile (#1567).
    launch::pin_profile_for_descendants(&mut cmd, &dir);
    Ok((cmd, dir))
}

/// The `claude` binary `login` and `usage` run directly (honouring
/// `AMPLIHACK_CLAUDE_BINARY_PATH`; the launcher's binary selection is not
/// used).
pub(super) fn resolve_claude_binary() -> Result<BinaryInfo> {
    let target = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    )
    .target
    .context("could not find a working claude binary; install Claude Code first")?;
    Ok(BinaryInfo {
        name: "claude".into(),
        path: target.path,
        version: Some(target.version),
    })
}

pub(super) fn login(store: &ProfileStore, name: &ProfileName) -> Result<()> {
    let binary = resolve_claude_binary()?;
    let status = build_profile_command(&binary, store, name, &[])
        .map_err(super::with_hint)?
        .status()
        .context("failed to run claude")?;
    if !status.success() {
        anyhow::bail!("claude exited with {status}");
    }
    Ok(())
}

pub(super) fn run(store: &ProfileStore, name: &ProfileName, args: Vec<String>) -> Result<()> {
    super::reject_docker_env()?;
    let dir = store.resolve(name).map_err(super::with_hint)?;
    launch::run_launch_profiled(
        "claude",
        "claude",
        false,
        false,
        false,
        true,
        false,
        false,
        false,
        None,
        args,
        amplihack_utils::launch_target::OverrideOrigin::User,
        Some(amplihack_utils::litellm_proxy::CliTarget::Claude),
        Some(dir),
    )
}
