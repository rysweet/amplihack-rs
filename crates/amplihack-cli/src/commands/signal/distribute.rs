//! Fleet distribute state model & resumable rollout planning (#923).
//!
//! Per-VM rollout state is tracked in an atomic JSON file
//! (`~/.amplihack/signal-distribute-state.json`) so a rollout is **resumable**
//! and **isolating**: a re-run skips VMs that reached terminal success and
//! retries the rest, and one VM failing never drops or aborts the others.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::seams::VmLister;

/// Current on-disk schema version. A file claiming a higher version is refused
/// (guards against a newer writer's data being silently downgraded).
pub const SCHEMA_VERSION: u32 = 1;

/// Per-VM onboarding status. Only [`VmStatus::ConfigWritten`] is terminal
/// success; every other state is retried on a resumed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VmStatus {
    /// Never attempted (or reset).
    Pending,
    /// Device linking is in progress.
    Linking,
    /// Device linked, daemon not yet confirmed.
    Linked,
    /// Local signal-cli daemon running, config not yet written.
    DaemonRunning,
    /// Fully onboarded: config written. **Terminal success.**
    ConfigWritten,
    /// Onboarding failed; `reason` explains why. Retried on resume.
    Failed,
}

impl VmStatus {
    /// Whether this status is the single terminal-success state.
    pub fn is_terminal_success(&self) -> bool {
        matches!(self, VmStatus::ConfigWritten)
    }
}

/// A single VM's tracked record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmRecord {
    /// Current status.
    pub status: VmStatus,
    /// Optional human-readable reason (typically set for failures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// RFC3339 timestamp of the last update. Never empty.
    pub updated_at: String,
}

/// The full rollout state: a schema version plus per-VM records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributeState {
    /// On-disk schema version.
    pub version: u32,
    /// VM name → record. `BTreeMap` for deterministic serialization.
    pub vms: BTreeMap<String, VmRecord>,
}

impl Default for DistributeState {
    fn default() -> Self {
        Self::new()
    }
}

impl DistributeState {
    /// A fresh, empty state at the current schema version.
    pub fn new() -> Self {
        Self {
            version: SCHEMA_VERSION,
            vms: BTreeMap::new(),
        }
    }

    /// Insert or update a VM's status + reason, stamping `updated_at`.
    pub fn upsert(&mut self, vm: &str, status: VmStatus, reason: Option<String>) {
        self.vms.insert(
            vm.to_string(),
            VmRecord {
                status,
                reason,
                updated_at: now_rfc3339(),
            },
        );
    }

    /// Look up a VM's record.
    pub fn get(&self, vm: &str) -> Option<&VmRecord> {
        self.vms.get(vm)
    }

    /// From the full desired VM set, the targets that still need work: every VM
    /// that has not reached terminal success (unseen, failed, or mid-flight).
    pub fn resumable_targets(&self, all: &[String]) -> Vec<String> {
        all.iter()
            .filter(|vm| {
                self.vms
                    .get(*vm)
                    .map(|rec| !rec.status.is_terminal_success())
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Count of VMs at terminal success.
    pub fn succeeded_count(&self) -> usize {
        self.vms
            .values()
            .filter(|r| r.status.is_terminal_success())
            .count()
    }

    /// Count of VMs in the failed state.
    pub fn failed_count(&self) -> usize {
        self.vms
            .values()
            .filter(|r| r.status == VmStatus::Failed)
            .count()
    }

    /// Persist the state atomically, `0600` (secrets-adjacent).
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("serialize distribute state")?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create state dir {}", parent.display()))?;
        }
        write_private(path, json.as_bytes())
            .with_context(|| format!("write state file {}", path.display()))?;
        Ok(())
    }

    /// Load state, refusing an unknown (higher) schema version.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read state file {}", path.display()))?;
        let state: DistributeState =
            serde_json::from_slice(&bytes).context("parse distribute state JSON")?;
        if state.version > SCHEMA_VERSION {
            anyhow::bail!(
                "distribute state schema version {} is newer than supported {} — refusing to load (upgrade amplihack)",
                state.version,
                SCHEMA_VERSION
            );
        }
        Ok(state)
    }
}

/// Plan a fleet rollout: enumerate VMs via the injected [`VmLister`] and return
/// the resumable targets. A discovery failure propagates (no silent fallback).
pub fn plan_rollout(
    lister: &impl VmLister,
    state: &DistributeState,
    resource_group: &str,
) -> Result<Vec<String>> {
    let all = lister
        .list_vms(resource_group)
        .context("VM discovery failed")?;
    Ok(state.resumable_targets(&all))
}

/// Current time as an RFC3339 string.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Write bytes to `path` with `0600` permissions on Unix (mode enforced at
/// create time so it is umask-independent).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        // Enforce mode even if the file pre-existed with looser perms.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
