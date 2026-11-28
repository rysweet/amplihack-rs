//! Native Codex package and exact-owned user hook registration.
//! Canonical skills remain provider-neutral; generated instruction skills are
//! installation outputs. Native trust is deliberately left to Codex.
use super::paths::home_dir;
use amplihack_utils::list_selection::{ListSelection, ListSet};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion: Option<completion_proof::Completion>,
    #[serde(default, skip_serializing_if = "is_false")]
    uninstall_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uninstall_inventory: Option<Value>,
    codex_home: PathBuf,
    package_digest: String,
    hooks: Value,
}
fn is_false(value: &bool) -> bool {
    !value
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
mod completion_proof;
mod config;
mod config_diff;
mod package_cleanup;
mod recovery;
mod recovery_archive;
mod recovery_archive_intent;
mod recovery_archive_storage;
mod recovery_staleness;
mod rollback;
mod rollback_durability;
use recovery::*;
mod packaging;
use packaging::*;
mod planning;
use planning::*;
mod lock_file;
use lock_file::{open_lock_file, open_package_lock};
mod deny_rules;
mod path_scope;
#[cfg(all(test, unix))]
mod path_scope_tests;
mod transaction;
mod uninstall;
pub(super) use deny_rules::{DenyRulesRemoval, DenyRulesWrite};
// The layout of the rules file is checked by the install tests; production
// code reaches it only through the two functions below.
#[cfg(test)]
pub(super) use deny_rules::{RULES_DIR, RULES_FILE};
pub(super) fn install(source: &Path, hooks_binary: &Path) -> Result<bool> {
    transaction::install(source, hooks_binary)
}
pub(super) fn uninstall() -> Result<()> {
    // Issue #2589: amplihack's own rules file goes with the installation;
    // anything else in that directory stays, including a rules file someone
    // wrote by hand at the same path.
    match uninstall_deny_rules() {
        DenyRulesRemoval::Removed | DenyRulesRemoval::Absent => {}
        DenyRulesRemoval::Kept(reason) => {
            println!("  ℹ️  The Codex dangerous-command rules file was left in place ({reason}).")
        }
    }
    uninstall::uninstall()
}
pub(super) fn reset_recovery() -> Result<()> {
    recovery_archive::reset()
}

/// Issue #2589: writes amplihack's dangerous-command rules file into the
/// Codex home this process manages, inside the installer's path scope, so an
/// escaping or symlinked directory component is caught before any write.
/// Never fails the install: every outcome, including a refusal, is returned.
pub(super) fn install_deny_rules() -> DenyRulesWrite {
    let instance = crate::user_config::policy_deny_commands();
    match in_codex_scope(|home| deny_rules::write_deny_rules(home, &instance)) {
        Ok(outcome) => outcome,
        Err(reason) => DenyRulesWrite::Refused(reason),
    }
}

/// Issue #2589: removes the rules file amplihack wrote, and only that file.
pub(super) fn uninstall_deny_rules() -> DenyRulesRemoval {
    match in_codex_scope(deny_rules::remove_deny_rules) {
        Ok(outcome) => outcome,
        Err(reason) => DenyRulesRemoval::Kept(reason),
    }
}

/// Runs `action` against this process's Codex home inside the installer's
/// path scope, so an escaping or symlinked directory component is caught
/// before any write. A scope that cannot be captured is a plain phrase, not
/// an error: a deny list that could not be written must be reported, and
/// must not stop the rest of the install.
fn in_codex_scope<T>(action: impl FnOnce(&Path) -> T) -> std::result::Result<T, String> {
    let root = root().map_err(|error| format!("the staging root is unknown ({error})"))?;
    let home = codex_home().map_err(|error| format!("the Codex home is unknown ({error})"))?;
    path_scope::with_scope(&root, &home, || Ok(action(&home)))
        .map_err(|error| format!("the Codex home could not be checked ({error})"))
}
#[cfg(test)]
mod backup_cleanup_tests;
#[cfg(all(test, unix))]
mod capability_tests;
#[cfg(all(test, unix))]
mod classify_tests;
#[cfg(test)]
mod cleanup_performance_tests;
#[cfg(all(test, unix))]
mod completion_proof_tests;
#[cfg(all(test, unix))]
mod config_layout_tests;
#[cfg(all(test, unix))]
mod deny_rules_tests;
#[cfg(test)]
mod digest_tests;
#[cfg(test)]
mod durability_tests;
#[cfg(all(test, unix))]
mod idempotence_tests;
#[cfg(test)]
mod legacy_migration_tests;
#[cfg(all(test, unix))]
mod probe_install_tests;
#[cfg(all(test, unix))]
mod probe_tests;
#[cfg(all(test, unix))]
mod recovery_alias_tests;
#[cfg(all(test, unix))]
mod recovery_archive_guard_tests;
#[cfg(all(test, unix))]
mod recovery_archive_retry_tests;
#[cfg(all(test, unix))]
mod recovery_archive_tests;
#[cfg(all(test, unix))]
mod recovery_staleness_tests;
#[cfg(all(test, unix))]
mod recovery_test_support;
#[cfg(test)]
mod recovery_tests;
#[cfg(all(test, unix))]
mod rollback_durability_tests;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod tests;

pub(super) fn with_binary<T>(binary: &Path, action: impl FnOnce() -> T) -> T {
    native::with_binary(binary, action)
}
