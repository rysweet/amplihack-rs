//! `profile login` and `profile run`.

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use crate::binary_finder::BinaryInfo;
use crate::commands::launch;
use anyhow::{Context, Result};
use std::process::Command;

/// Builds the child command with the profile directory verified at call time.
/// Arguments are an argv vector; no shell is involved.
pub(super) fn build_profile_command(
    binary: &BinaryInfo,
    store: &ProfileStore,
    name: &ProfileName,
    args: &[String],
) -> Result<Command, ProfileError> {
    let dir = store.resolve(name)?;
    let mut cmd = Command::new(&binary.path);
    cmd.args(args).env("CLAUDE_CONFIG_DIR", dir);
    Ok(cmd)
}

pub(super) fn login(store: &ProfileStore, name: &ProfileName) -> Result<()> {
    let target = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    )
    .target
    .context("could not find a working claude binary; install Claude Code first")?;
    let binary = BinaryInfo {
        name: "claude".into(),
        path: target.path,
        version: Some(target.version),
    };
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
