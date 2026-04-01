//! amplihack-fleet: Remote fleet infrastructure for distributed agent orchestration.
//!
//! Provides VM state tracking, health checking, credential propagation,
//! transcript analysis, and persistent task queue — matching the Python
//! amplihack fleet subsystem's infrastructure layer.

pub mod auth;
pub mod health;
pub mod task_queue;
pub mod transcript;
pub mod vm_state;

pub use auth::{CredentialInventory, CredentialType, PropagationResult};
pub use health::{HealthReport, HealthStatus, ResourceInfo};
pub use task_queue::{FleetTask, Priority, TaskQueue, TaskStatus};
pub use transcript::{TranscriptEntry, TranscriptReport, WorkflowCompliance};
pub use vm_state::{FleetState, SessionStatus, TmuxSessionInfo, VmInfo, VmStatus};
