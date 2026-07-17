//! VM Pool management for multi-session capacity.
//!
//! Tracks Azure VMs and their concurrent session capacity, enabling
//! efficient VM reuse across multiple sessions.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::error::RemoteError;
use crate::orchestrator::{Orchestrator, VM, VMOptions};
use crate::state_io::{merge_key_into_state, read_keyed_state};

/// VM capacity tiers for concurrent sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VMSize {
    /// 1 concurrent session (32 GB VM)
    S = 1,
    /// 2 concurrent sessions (64 GB VM)
    M = 2,
    /// 4 concurrent sessions (128 GB VM)
    L = 4,
    /// 8 concurrent sessions (256 GB VM)
    XL = 8,
}

impl VMSize {
    /// Number of concurrent sessions this size supports.
    pub fn capacity(self) -> usize {
        self as usize
    }

    /// Map to Azure VM SKU.
    pub fn azure_size(self) -> &'static str {
        match self {
            Self::S => "Standard_D8s_v3",
            Self::M => "Standard_E8s_v5",
            Self::L => "Standard_E16s_v5",
            Self::XL => "Standard_E32s_v5",
        }
    }
}

impl fmt::Display for VMSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
            Self::XL => "xl",
        })
    }
}

impl FromStr for VMSize {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "s" | "S" => Ok(Self::S),
            "m" | "M" => Ok(Self::M),
            "l" | "L" => Ok(Self::L),
            "xl" | "XL" => Ok(Self::XL),
            _ => Err(format!("invalid VM size tier: {raw}")),
        }
    }
}

/// A VM in the pool with capacity tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VMPoolEntry {
    pub vm: VM,
    pub capacity: usize,
    pub active_sessions: Vec<String>,
    pub region: String,
}

impl VMPoolEntry {
    /// Remaining capacity.
    pub fn available_capacity(&self) -> usize {
        self.capacity.saturating_sub(self.active_sessions.len())
    }
}

/// Manages the VM pool for multi-session capacity.
pub struct VMPoolManager {
    state_file: PathBuf,
    pool: HashMap<String, VMPoolEntry>,
    orchestrator: Orchestrator,
}

impl VMPoolManager {
    /// Create a new pool manager loading state from disk.
    pub fn new(
        state_file: Option<PathBuf>,
        orchestrator: Orchestrator,
    ) -> Result<Self, RemoteError> {
        let state_file =
            state_file.unwrap_or_else(|| dirs_home().join(".amplihack").join("remote-state.json"));

        let mut mgr = Self {
            state_file,
            pool: HashMap::new(),
            orchestrator,
        };
        mgr.load_state()?;
        Ok(mgr)
    }

    /// Allocate a VM for a session (reuse or provision new).
    pub async fn allocate_vm(
        &mut self,
        session_id: &str,
        size: VMSize,
        region: &str,
    ) -> Result<VM, RemoteError> {
        if session_id.trim().is_empty() {
            return Err(RemoteError::provisioning("session_id cannot be empty"));
        }

        // Try to find a VM with available capacity
        let found_vm = {
            let mut result = None;
            for entry in self.pool.values_mut() {
                if entry.region != region {
                    continue;
                }
                if entry.available_capacity() == 0 {
                    continue;
                }

                info!(
                    vm = %entry.vm.name,
                    session = session_id,
                    "reusing VM from pool"
                );
                entry.active_sessions.push(session_id.to_string());
                result = Some(entry.vm.clone());
                break;
            }
            result
        };

        if let Some(vm) = found_vm {
            self.save_state()?;
            return Ok(vm);
        }

        // Provision new VM
        info!(
            size = ?size,
            region,
            "provisioning new VM for pool"
        );
        let options = VMOptions {
            size: size.azure_size().to_string(),
            region: Some(region.to_string()),
            no_reuse: false,
            ..VMOptions::default()
        };

        let vm = self.orchestrator.provision_or_reuse(&options).await?;

        let entry = VMPoolEntry {
            vm: vm.clone(),
            capacity: size.capacity(),
            active_sessions: vec![session_id.to_string()],
            region: region.to_string(),
        };
        self.pool.insert(vm.name.clone(), entry);
        self.save_state()?;
        Ok(vm)
    }

    /// Release a session from its VM.
    pub fn release_session(&mut self, session_id: &str) {
        for entry in self.pool.values_mut() {
            if let Some(pos) = entry.active_sessions.iter().position(|s| s == session_id) {
                entry.active_sessions.remove(pos);
                info!(
                    session = session_id,
                    vm = %entry.vm.name,
                    "session released"
                );
                let _ = self.save_state();
                return;
            }
        }
        debug!(session = session_id, "session not found in pool");
    }

    /// Get pool status summary.
    pub fn get_pool_status(&self) -> PoolStatus {
        let total_vms = self.pool.len();
        let total_capacity: usize = self.pool.values().map(|e| e.capacity).sum();
        let active_sessions: usize = self.pool.values().map(|e| e.active_sessions.len()).sum();
        let available_capacity: usize = self.pool.values().map(|e| e.available_capacity()).sum();

        PoolStatus {
            total_vms,
            total_capacity,
            active_sessions,
            available_capacity,
        }
    }

    /// Cleanup idle VMs older than `grace_period_minutes`.
    pub async fn cleanup_idle_vms(&mut self, grace_period_minutes: i64) -> Vec<String> {
        let now = Utc::now();
        let grace = Duration::minutes(grace_period_minutes);
        let mut removed = Vec::new();

        let idle_vms: Vec<String> = self
            .pool
            .iter()
            .filter(|(_, entry)| {
                if !entry.active_sessions.is_empty() {
                    return false;
                }
                if let Some(created) = entry.vm.created_at {
                    now - created >= grace
                } else {
                    true
                }
            })
            .map(|(name, _)| name.clone())
            .collect();

        for vm_name in idle_vms {
            if let Some(entry) = self.pool.remove(&vm_name) {
                // Take ownership of the entry (ends the &mut self.pool borrow)
                // and await cleanup borrowing only &self.orchestrator, so the
                // helper below can re-borrow &mut self.pool without conflict.
                let result = self.orchestrator.cleanup(&entry.vm, true).await;
                Self::apply_cleanup_result(&mut self.pool, &mut removed, vm_name, entry, result);
            }
        }

        if !removed.is_empty() {
            info!(count = removed.len(), "cleaned up idle VMs");
            let _ = self.save_state();
        }

        removed
    }

    /// Map one `Orchestrator::cleanup` outcome onto the pool and the `removed`
    /// list, distinguishing a confirmed reclaim from a failure so a billable
    /// VM is never silently dropped from tracking (issue #870).
    ///
    /// Outcomes are inspected structurally from `Result<bool, RemoteError>` —
    /// never by parsing tool output:
    /// - `Ok(true)`  — deallocation confirmed: drop the VM and record it in
    ///   `removed` (the only truthful "reclaimed" signal).
    /// - `Ok(false)` — cleanup ran but did not confirm deallocation: re-insert
    ///   the entry so the next pass retries; do not report it as removed.
    /// - `Err(e)`    — hard cleanup failure: re-insert the entry to avoid
    ///   orphaning a billable resource; do not report it as removed.
    fn apply_cleanup_result(
        pool: &mut HashMap<String, VMPoolEntry>,
        removed: &mut Vec<String>,
        vm_name: String,
        entry: VMPoolEntry,
        result: Result<bool, RemoteError>,
    ) {
        match result {
            Ok(true) => {
                removed.push(vm_name);
            }
            Ok(false) => {
                warn!(
                    vm = %vm_name,
                    "cleanup did not confirm deallocation; VM retained for retry"
                );
                pool.insert(vm_name, entry);
            }
            Err(e) => {
                // Log only the curated RemoteError Display message (never raw
                // tool stdout/stderr) plus the VM name — no credentials, tags,
                // or command output are surfaced here.
                error!(
                    vm = %vm_name,
                    error = %e,
                    "cleanup failed; VM retained to avoid orphaning billable resource"
                );
                pool.insert(vm_name, entry);
            }
        }
    }

    // ---- state persistence ----

    fn load_state(&mut self) -> Result<(), RemoteError> {
        // Missing/empty/absent-key → start empty; corrupt or schema mismatch →
        // surface, never discard.
        self.pool = read_keyed_state(&self.state_file, "vm_pool")
            .map_err(|e| RemoteError::packaging(e.to_string()))?
            .unwrap_or_default();
        Ok(())
    }

    fn save_state(&self) -> Result<(), RemoteError> {
        let pool_json = serde_json::to_value(&self.pool)
            .map_err(|e| RemoteError::packaging(format!("Failed to serialize pool: {e}")))?;
        // Merges under lock and refuses to overwrite a corrupt file, so
        // co-resident session state is never wiped.
        merge_key_into_state(&self.state_file, "vm_pool", pool_json)
            .map_err(|e| RemoteError::packaging(e.to_string()))
    }
}

/// Pool status summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStatus {
    pub total_vms: usize,
    pub total_capacity: usize,
    pub active_sessions: usize,
    pub available_capacity: usize,
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
}

#[cfg(test)]
#[path = "vm_pool_tests.rs"]
mod tests;
