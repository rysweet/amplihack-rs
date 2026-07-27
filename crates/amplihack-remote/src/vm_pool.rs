//! VM Pool management for multi-session capacity.
//!
//! Tracks Azure VMs and their concurrent session capacity, enabling
//! efficient VM reuse across multiple sessions.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

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
                if let Err(e) = self.save_state() {
                    warn!(
                        session = session_id,
                        error = %e,
                        "failed to persist pool state after releasing session; \
                         in-memory and on-disk state may diverge"
                    );
                }
                return;
            }
        }
        debug!(session = session_id, "session not found in pool");
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
