//! amplihack-remote: Azure VM remote-execution pipeline.
//!
//! Provides the full pipeline for packaging project context, provisioning
//! Azure VMs via azlin, executing amplihack commands remotely, and
//! integrating results back into the local repository.
//!
//! # Modules
//!
//! - [`auth`] — Azure Service Principal authentication
//! - [`packager`] — Context packaging with secret scanning
//! - [`orchestrator`] — VM lifecycle management via azlin
//! - [`executor`] — Remote command execution (SCP/SSH)
//! - [`integrator`] — Result integration (git fetch, merge)
//! - [`vm_pool`] — Multi-session VM pool management
//! - [`session`] — Remote session lifecycle management
//! - [`state_lock`] — Advisory file locking
//! - [`cli`] — Full workflow orchestration
//! - [`error`] — Error types

pub mod auth;
pub(crate) mod azlin_parse;
pub mod cli;
pub mod commands;
pub mod error;
pub mod executor;
pub mod integrator;
pub mod orchestrator;
pub mod packager;
pub mod session;
pub mod state_lock;
pub mod vm_pool;

pub use auth::{AzureAuthenticator, AzureCredentials, get_azure_auth};
pub use cli::{
    WorkflowOptions, WorkflowResult, execute_remote_workflow, execute_remote_workflow_with_api_key,
};
pub use commands::{
    CommandMode, ExecOptions, KillOptions, ListOptions, OutputOptions, OutputResult, RemoteStatus,
    SessionCounts, StartOptions, StartSummary, StatusOptions, capture_output, exec, kill_session,
    list_sessions, start_sessions, status,
};
pub use error::{ErrorContext, RemoteError};
pub use executor::{ExecutionResult, Executor};
pub use integrator::{BranchInfo, IntegrationSummary, Integrator};
pub use orchestrator::{Orchestrator, VM, VMOptions};
pub use packager::{ContextPackager, SecretMatch};
pub use session::{Session, SessionManager, SessionStatus};
pub use state_lock::{FileLockGuard, file_lock};
pub use vm_pool::{PoolStatus, VMPoolEntry, VMPoolManager, VMSize};
