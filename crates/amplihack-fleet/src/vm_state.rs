//! VM and session state tracking for remote fleet orchestration.
//!
//! Matches Python `amplihack/fleet/fleet_state.py`:
//! - VMInfo: Azure VM metadata and connectivity
//! - TmuxSessionInfo: Remote tmux session state
//! - FleetState: Real-time inventory of VMs and sessions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Status of a VM in the fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmStatus {
    Running,
    Stopped,
    Deallocated,
    Unknown,
    Error,
}

/// Information about an Azure VM in the fleet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub name: String,
    pub resource_group: String,
    pub ip_address: Option<String>,
    pub status: VmStatus,
    pub ssh_user: String,
    pub region: String,
    pub last_polled: f64,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

impl VmInfo {
    pub fn new(name: impl Into<String>, resource_group: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            resource_group: resource_group.into(),
            ip_address: None,
            status: VmStatus::Unknown,
            ssh_user: "azureuser".into(),
            region: String::new(),
            last_polled: 0.0,
            tags: HashMap::new(),
        }
    }

    /// SSH connection string for this VM.
    pub fn ssh_target(&self) -> Option<String> {
        self.ip_address
            .as_ref()
            .map(|ip| format!("{}@{}", self.ssh_user, ip))
    }

    /// Whether the VM is reachable (running with an IP).
    pub fn is_reachable(&self) -> bool {
        self.status == VmStatus::Running && self.ip_address.is_some()
    }
}

/// Status of a tmux session on a remote VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Idle,
    Completed,
    Failed,
    Unknown,
}

/// Information about a tmux session on a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmuxSessionInfo {
    pub session_name: String,
    pub vm_name: String,
    pub status: SessionStatus,
    pub agent_type: String,
    pub working_dir: String,
    pub created_at: f64,
    pub last_activity: f64,
}

impl TmuxSessionInfo {
    pub fn new(
        session_name: impl Into<String>,
        vm_name: impl Into<String>,
        agent_type: impl Into<String>,
    ) -> Self {
        let now = now_secs();
        Self {
            session_name: session_name.into(),
            vm_name: vm_name.into(),
            status: SessionStatus::Unknown,
            agent_type: agent_type.into(),
            working_dir: String::new(),
            created_at: now,
            last_activity: now,
        }
    }

    /// Whether this session is actively running.
    pub fn is_active(&self) -> bool {
        matches!(self.status, SessionStatus::Active | SessionStatus::Idle)
    }
}

/// Real-time inventory of VMs and sessions in the fleet.
pub struct FleetState {
    vms: HashMap<String, VmInfo>,
    sessions: Vec<TmuxSessionInfo>,
}

impl FleetState {
    pub fn new() -> Self {
        Self {
            vms: HashMap::new(),
            sessions: Vec::new(),
        }
    }

    /// Register or update a VM.
    pub fn upsert_vm(&mut self, vm: VmInfo) {
        self.vms.insert(vm.name.clone(), vm);
    }

    /// Remove a VM and its sessions.
    pub fn remove_vm(&mut self, name: &str) {
        self.vms.remove(name);
        self.sessions.retain(|s| s.vm_name != name);
    }

    /// Get a VM by name.
    pub fn get_vm(&self, name: &str) -> Option<&VmInfo> {
        self.vms.get(name)
    }

    /// All VMs.
    pub fn vms(&self) -> impl Iterator<Item = &VmInfo> {
        self.vms.values()
    }

    /// Register or update a session.
    pub fn upsert_session(&mut self, session: TmuxSessionInfo) {
        if let Some(existing) = self
            .sessions
            .iter_mut()
            .find(|s| s.session_name == session.session_name && s.vm_name == session.vm_name)
        {
            *existing = session;
        } else {
            self.sessions.push(session);
        }
    }

    /// Sessions on a specific VM.
    pub fn sessions_on_vm(&self, vm_name: &str) -> Vec<&TmuxSessionInfo> {
        self.sessions
            .iter()
            .filter(|s| s.vm_name == vm_name)
            .collect()
    }

    /// All active sessions.
    pub fn active_sessions(&self) -> Vec<&TmuxSessionInfo> {
        self.sessions.iter().filter(|s| s.is_active()).collect()
    }

    /// Count of reachable VMs.
    pub fn reachable_vm_count(&self) -> usize {
        self.vms.values().filter(|vm| vm.is_reachable()).count()
    }

    /// Total session count.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// VM count.
    pub fn vm_count(&self) -> usize {
        self.vms.len()
    }
}

impl Default for FleetState {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_info_basics() {
        let mut vm = VmInfo::new("vm-1", "rg-fleet");
        assert!(!vm.is_reachable());
        vm.status = VmStatus::Running;
        vm.ip_address = Some("10.0.0.1".into());
        assert!(vm.is_reachable());
        assert_eq!(vm.ssh_target(), Some("azureuser@10.0.0.1".into()));
    }

    #[test]
    fn fleet_state_vm_operations() {
        let mut state = FleetState::new();
        state.upsert_vm(VmInfo::new("vm-1", "rg"));
        state.upsert_vm(VmInfo::new("vm-2", "rg"));
        assert_eq!(state.vm_count(), 2);
        state.remove_vm("vm-1");
        assert_eq!(state.vm_count(), 1);
    }

    #[test]
    fn fleet_state_session_tracking() {
        let mut state = FleetState::new();
        let mut session = TmuxSessionInfo::new("agent-0", "vm-1", "claude");
        session.status = SessionStatus::Active;
        state.upsert_session(session);
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.active_sessions().len(), 1);
        assert_eq!(state.sessions_on_vm("vm-1").len(), 1);
        assert_eq!(state.sessions_on_vm("vm-2").len(), 0);
    }

    #[test]
    fn upsert_session_updates_existing() {
        let mut state = FleetState::new();
        let mut s1 = TmuxSessionInfo::new("agent-0", "vm-1", "claude");
        s1.status = SessionStatus::Active;
        state.upsert_session(s1);
        let mut s2 = TmuxSessionInfo::new("agent-0", "vm-1", "claude");
        s2.status = SessionStatus::Completed;
        state.upsert_session(s2);
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.active_sessions().len(), 0);
    }

    #[test]
    fn vm_serializes() {
        let vm = VmInfo::new("test", "rg");
        let json = serde_json::to_value(&vm).unwrap();
        assert_eq!(json["name"], "test");
        assert_eq!(json["status"], "unknown");
    }

    #[test]
    fn remove_vm_removes_sessions() {
        let mut state = FleetState::new();
        state.upsert_vm(VmInfo::new("vm-1", "rg"));
        state.upsert_session(TmuxSessionInfo::new("s1", "vm-1", "claude"));
        state.upsert_session(TmuxSessionInfo::new("s2", "vm-1", "claude"));
        state.remove_vm("vm-1");
        assert_eq!(state.session_count(), 0);
    }

    #[test]
    fn remove_nonexistent_vm_is_noop() {
        let mut state = FleetState::new();
        state.upsert_vm(VmInfo::new("vm-1", "rg"));
        state.remove_vm("vm-999");
        assert_eq!(state.vm_count(), 1);
    }

    #[test]
    fn reachable_vm_count() {
        let mut state = FleetState::new();
        let mut vm1 = VmInfo::new("vm-1", "rg");
        vm1.status = VmStatus::Running;
        vm1.ip_address = Some("10.0.0.1".into());
        state.upsert_vm(vm1);
        state.upsert_vm(VmInfo::new("vm-2", "rg")); // unreachable
        assert_eq!(state.reachable_vm_count(), 1);
    }

    #[test]
    fn vm_ssh_target_none_without_ip() {
        let vm = VmInfo::new("vm-1", "rg");
        assert!(vm.ssh_target().is_none());
    }

    #[test]
    fn session_is_active_variants() {
        let mut s = TmuxSessionInfo::new("s1", "vm-1", "claude");
        s.status = SessionStatus::Active;
        assert!(s.is_active());
        s.status = SessionStatus::Idle;
        assert!(s.is_active(), "Idle is considered active");
        s.status = SessionStatus::Completed;
        assert!(!s.is_active());
        s.status = SessionStatus::Failed;
        assert!(!s.is_active());
    }

    #[test]
    fn fleet_state_counts_are_consistent() {
        let mut state = FleetState::new();
        state.upsert_vm(VmInfo::new("vm-1", "rg"));
        state.upsert_vm(VmInfo::new("vm-2", "rg"));
        state.upsert_session(TmuxSessionInfo::new("s1", "vm-1", "claude"));
        assert_eq!(state.vm_count(), 2);
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.vms().count(), 2);
    }
}
