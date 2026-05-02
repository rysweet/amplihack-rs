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
use tracing::{debug, info};

use crate::error::RemoteError;
use crate::orchestrator::{Orchestrator, VM, VMOptions};
use crate::state_lock::file_lock;

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
                let _ = self.orchestrator.cleanup(&entry.vm, true).await;
                removed.push(vm_name);
            }
        }

        if !removed.is_empty() {
            info!(count = removed.len(), "cleaned up idle VMs");
            let _ = self.save_state();
        }

        removed
    }

    // ---- state persistence ----

    fn load_state(&mut self) -> Result<(), RemoteError> {
        if !self.state_file.exists() {
            self.pool = HashMap::new();
            return Ok(());
        }

        let content = std::fs::read_to_string(&self.state_file)
            .map_err(|e| RemoteError::packaging(format!("Failed to read state: {e}")))?;

        if content.trim().is_empty() {
            self.pool = HashMap::new();
            return Ok(());
        }

        let data: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| RemoteError::packaging(format!("State file corrupt: {e}")))?;

        if let Some(pool_data) = data.get("vm_pool") {
            let entries: HashMap<String, VMPoolEntry> =
                serde_json::from_value(pool_data.clone()).unwrap_or_default();
            self.pool = entries;
        }

        Ok(())
    }

    fn save_state(&self) -> Result<(), RemoteError> {
        let lock_path = self.state_file.with_extension("lock");
        let _guard = file_lock(&lock_path)
            .map_err(|e| RemoteError::packaging(format!("Failed to acquire lock: {e}")))?;

        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| RemoteError::packaging(format!("Failed to create state dir: {e}")))?;
        }

        // Load existing state to merge
        let mut existing: serde_json::Value = if self.state_file.exists() {
            std::fs::read_to_string(&self.state_file)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_else(|| serde_json::json!({"sessions": {}}))
        } else {
            serde_json::json!({"sessions": {}})
        };

        let pool_json = serde_json::to_value(&self.pool)
            .map_err(|e| RemoteError::packaging(format!("Failed to serialize pool: {e}")))?;

        existing["vm_pool"] = pool_json;

        let content = serde_json::to_string_pretty(&existing)
            .map_err(|e| RemoteError::packaging(format!("Failed to serialize state: {e}")))?;

        std::fs::write(&self.state_file, content)
            .map_err(|e| RemoteError::packaging(format!("Failed to write state: {e}")))?;

        Ok(())
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
mod tests {
    use super::*;

    #[test]
    fn vm_size_capacity() {
        assert_eq!(VMSize::S.capacity(), 1);
        assert_eq!(VMSize::M.capacity(), 2);
        assert_eq!(VMSize::L.capacity(), 4);
        assert_eq!(VMSize::XL.capacity(), 8);
    }

    #[test]
    fn vm_size_azure_mapping() {
        assert_eq!(VMSize::S.azure_size(), "Standard_D8s_v3");
        assert_eq!(VMSize::XL.azure_size(), "Standard_E32s_v5");
    }

    #[test]
    fn pool_entry_available_capacity() {
        let entry = VMPoolEntry {
            vm: VM {
                name: "vm1".into(),
                size: "s".into(),
                region: "eastus".into(),
                created_at: None,
                tags: None,
            },
            capacity: 4,
            active_sessions: vec!["s1".into(), "s2".into()],
            region: "eastus".into(),
        };
        assert_eq!(entry.available_capacity(), 2);
    }

    #[test]
    fn pool_status_serialization() {
        let status = PoolStatus {
            total_vms: 2,
            total_capacity: 8,
            active_sessions: 3,
            available_capacity: 5,
        };
        let json = serde_json::to_string(&status).unwrap();
        let s2: PoolStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s2.total_vms, 2);
    }
}
