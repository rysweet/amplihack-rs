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
pub mod azlin_parse;
pub(crate) mod backoff;
pub mod cli;
pub mod commands;
pub mod error;
pub mod executor;
pub mod integrator;
pub mod orchestrator;
pub mod packager;
mod redact;
mod script;
pub mod session;
pub(crate) mod shell_safe;
pub mod state_io;
pub mod state_lock;
pub(crate) mod vm_lookup;
pub mod vm_pool;

pub use auth::{AzureAuthenticator, AzureCredentials, get_azure_auth};
// #921/#971 R4: re-export the azlin discovery parsers and the idle/liveness
// watchdog so the Signal fleet path (idle-based device-linking) can consume a
// single, shared implementation instead of forking the parse/idle logic.
pub use amplihack_utils::idle_watchdog;
pub use azlin_parse::{parse_azlin_list_json, parse_azlin_list_text};
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
