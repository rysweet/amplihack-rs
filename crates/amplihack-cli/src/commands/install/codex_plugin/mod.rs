//! Native Codex package and exact-owned user hook registration.
//! Canonical skills remain provider-neutral; generated instruction skills are
//! installation outputs. Native trust is deliberately left to Codex.
use super::paths::home_dir;
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const ID: &str = "amplihack@amplihack-local";
#[derive(Serialize, Deserialize)]
struct Ownership {
    schema_version: u32,
    #[serde(default)]
    transaction: Option<String>,
    codex_home: PathBuf,
    package_digest: String,
    hooks: Value,
}
fn root() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack/codex"))
}
fn codex_home() -> Result<PathBuf> {
    Ok(std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".codex")))
}

mod resources;
use resources::*;
mod instructions;
use instructions::*;
mod hooks;
use hooks::*;
mod native;
use native::*;
mod storage;
use storage::*;
mod backup_cleanup;
mod cleanup_guard;
mod config;
mod recovery;
mod rollback;
mod rollback_durability;
use recovery::*;
mod packaging;
use packaging::*;
mod planning;
use planning::*;
mod path_scope;
#[cfg(all(test, unix))]
mod path_scope_tests;
mod transaction;
mod uninstall;
pub(super) fn install(source: &Path, hooks_binary: &Path) -> Result<bool> {
    transaction::install(source, hooks_binary)
}
pub(super) fn uninstall() -> Result<()> {
    uninstall::uninstall()
}
#[cfg(test)]
mod backup_cleanup_tests;
#[cfg(test)]
mod cleanup_performance_tests;
#[cfg(test)]
mod digest_tests;
#[cfg(test)]
mod durability_tests;
#[cfg(all(test, unix))]
mod recovery_alias_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(all(test, unix))]
mod rollback_durability_tests;
#[cfg(test)]
mod tests;

pub(super) fn with_binary<T>(binary: &Path, action: impl FnOnce() -> T) -> T {
    native::with_binary(binary, action)
}
