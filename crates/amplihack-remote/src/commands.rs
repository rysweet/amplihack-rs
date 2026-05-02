//! Public command-level API for `amplihack remote`.

use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::cli::{WorkflowOptions, WorkflowResult, execute_remote_workflow_with_api_key};
use crate::error::RemoteError;
use crate::executor::Executor;
use crate::orchestrator::{Orchestrator, VMOptions};
use crate::packager::ContextPackager;
use crate::session::{Session, SessionManager, SessionStatus};
use crate::vm_pool::{PoolStatus, VMPoolEntry, VMPoolManager, VMSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandMode {
    Auto,
    Ultrathink,
    Analyze,
    Fix,
}

impl fmt::Display for CommandMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Ultrathink => "ultrathink",
            Self::Analyze => "analyze",
            Self::Fix => "fix",
        })
    }
}

impl FromStr for CommandMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "auto" => Ok(Self::Auto),
            "ultrathink" => Ok(Self::Ultrathink),
            "analyze" => Ok(Self::Analyze),
            "fix" => Ok(Self::Fix),
            _ => Err(format!("invalid command mode: {raw}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub repo_path: PathBuf,
    pub command: CommandMode,
    pub prompt: String,
    pub max_turns: u32,
    pub vm_options: VMOptions,
    pub timeout_minutes: u64,
    pub skip_secret_scan: bool,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub repo_path: PathBuf,
    pub prompts: Vec<String>,
    pub command: CommandMode,
    pub max_turns: u32,
    pub size: VMSize,
    pub region: Option<String>,
    pub tunnel_port: Option<u16>,
    pub api_key: String,
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub status: Option<SessionStatus>,
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct OutputOptions {
    pub session_id: String,
    pub lines: u32,
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct KillOptions {
    pub session_id: String,
    pub force: bool,
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct StatusOptions {
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartSummary {
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputResult {
    pub session: Session,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCounts {
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub killed: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteStatus {
    pub pool: PoolStatus,
    pub sessions: SessionCounts,
    pub total_sessions: usize,
    pub vms: Vec<VMPoolEntry>,
}

pub async fn exec(options: ExecOptions) -> Result<WorkflowResult, RemoteError> {
    validate_prompt(&options.prompt)?;
    validate_max_turns(options.max_turns)?;
    validate_timeout(options.timeout_minutes)?;
    validate_api_key(&options.api_key)?;
    execute_remote_workflow_with_api_key(WorkflowOptions {
        repo_path: &options.repo_path,
        command: &options.command.to_string(),
        prompt: &options.prompt,
        max_turns: options.max_turns,
        vm_options: &options.vm_options,
        timeout_minutes: options.timeout_minutes,
        skip_secret_scan: options.skip_secret_scan,
        api_key: &options.api_key,
    })
    .await
}

pub async fn start_sessions(options: StartOptions) -> Result<StartSummary, RemoteError> {
    if options.prompts.is_empty() {
        return Err(RemoteError::validation(
            "At least one prompt is required for remote start",
        ));
    }
    for prompt in &options.prompts {
        validate_prompt(prompt)?;
    }
    validate_max_turns(options.max_turns)?;
    validate_api_key(&options.api_key)?;

    let region = options
        .region
        .clone()
        .or_else(|| std::env::var("AZURE_REGION").ok())
        .unwrap_or_else(|| "eastus".to_string());
    let orchestrator = Orchestrator::new(None).await?;
    let mut sessions =
        SessionManager::new(options.state_file.clone()).map_err(RemoteError::packaging)?;
    let mut pool = VMPoolManager::new(options.state_file.clone(), orchestrator)?;
    let mut started = Vec::new();

    for prompt in &options.prompts {
        let mut packager = ContextPackager::new(&options.repo_path, 500, false);
        let archive = packager.package().await?;
        let session = sessions
            .create_session(
                "pending",
                prompt,
                Some(&options.command.to_string()),
                Some(options.max_turns),
                None,
            )
            .map_err(RemoteError::validation)?;
        let vm = pool
            .allocate_vm(&session.session_id, options.size, &region)
            .await?;
        let session = sessions
            .update_session_vm(&session.session_id, &vm.name)
            .map_err(RemoteError::packaging)?;
        let executor = Executor::new(vm, 120, options.tunnel_port);
        executor.transfer_context(&archive).await?;
        packager.cleanup();
        executor
            .execute_remote_tmux(
                &session.session_id,
                &options.command.to_string(),
                prompt,
                options.max_turns,
                &options.api_key,
            )
            .await?;
        sessions
            .start_session(&session.session_id)
            .map_err(RemoteError::packaging)?;
        started.push(session.session_id);
    }

    if started.is_empty() {
        Err(RemoteError::execution(
            "No sessions were started successfully",
        ))
    } else {
        Ok(StartSummary {
            session_ids: started,
        })
    }
}

pub fn list_sessions(options: ListOptions) -> Result<Vec<Session>, RemoteError> {
    let sessions = SessionManager::new(options.state_file).map_err(RemoteError::packaging)?;
    Ok(sessions
        .list_sessions(options.status)
        .into_iter()
        .cloned()
        .collect())
}

pub async fn capture_output(options: OutputOptions) -> Result<OutputResult, RemoteError> {
    if options.lines == 0 {
        return Err(RemoteError::validation("lines must be greater than 0"));
    }
    let sessions = SessionManager::new(options.state_file).map_err(RemoteError::packaging)?;
    let session = sessions
        .get_session(&options.session_id)
        .cloned()
        .ok_or_else(|| RemoteError::session_not_found(&options.session_id))?;
    let output = sessions
        .capture_output(&options.session_id, options.lines)
        .await;
    Ok(OutputResult { session, output })
}

pub async fn kill_session(options: KillOptions) -> Result<(), RemoteError> {
    let mut sessions =
        SessionManager::new(options.state_file.clone()).map_err(RemoteError::packaging)?;
    let session = sessions
        .get_session(&options.session_id)
        .cloned()
        .ok_or_else(|| RemoteError::session_not_found(&options.session_id))?;

    let kill_result = kill_tmux_session(&session.vm_name, &session.tmux_session).await;
    if let Err(err) = kill_result
        && !options.force
    {
        return Err(err);
    }

    if !sessions.kill_session(&options.session_id) {
        return Err(RemoteError::session_not_found(options.session_id));
    }
    release_session_in_state(options.state_file, &session.session_id)?;
    Ok(())
}

pub fn status(options: StatusOptions) -> Result<RemoteStatus, RemoteError> {
    let sessions = list_sessions(ListOptions {
        status: None,
        state_file: options.state_file.clone(),
    })?;
    let entries = load_pool_entries(options.state_file)?;
    let pool = pool_status_from_entries(&entries);
    let counts = SessionCounts {
        running: sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Running)
            .count(),
        completed: sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Completed)
            .count(),
        failed: sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Failed)
            .count(),
        killed: sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Killed)
            .count(),
        pending: sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Pending)
            .count(),
    };

    Ok(RemoteStatus {
        pool,
        sessions: counts,
        total_sessions: sessions.len(),
        vms: entries,
    })
}

fn validate_prompt(prompt: &str) -> Result<(), RemoteError> {
    if prompt.trim().is_empty() {
        Err(RemoteError::validation("prompt cannot be empty"))
    } else {
        Ok(())
    }
}

fn validate_max_turns(max_turns: u32) -> Result<(), RemoteError> {
    if (1..=50).contains(&max_turns) {
        Ok(())
    } else {
        Err(RemoteError::validation(
            "max-turns must be between 1 and 50",
        ))
    }
}

fn validate_timeout(timeout_minutes: u64) -> Result<(), RemoteError> {
    if (5..=480).contains(&timeout_minutes) {
        Ok(())
    } else {
        Err(RemoteError::validation(
            "timeout must be between 5 and 480 minutes",
        ))
    }
}

fn validate_api_key(api_key: &str) -> Result<(), RemoteError> {
    if api_key.trim().is_empty() {
        Err(RemoteError::validation(
            "ANTHROPIC_API_KEY not found in environment",
        ))
    } else {
        Ok(())
    }
}

async fn kill_tmux_session(vm_name: &str, tmux_session: &str) -> Result<(), RemoteError> {
    let command = format!("tmux kill-session -t {tmux_session}");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Command::new("azlin")
            .args(["connect", vm_name, &command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| RemoteError::execution("kill command timed out"))?
    .map_err(|e| RemoteError::execution(format!("kill command failed: {e}")))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(RemoteError::execution(format!(
            "Failed to kill tmux session: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn load_pool_entries(state_file: Option<PathBuf>) -> Result<Vec<VMPoolEntry>, RemoteError> {
    let state_file = state_file.unwrap_or_else(default_state_file);
    let data = read_state_json(&state_file)?;
    let entries = data
        .get("vm_pool")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| RemoteError::packaging(format!("State file corrupt: {e}")))?
        .unwrap_or_else(std::collections::HashMap::<String, VMPoolEntry>::new);
    Ok(entries.into_values().collect())
}

fn pool_status_from_entries(entries: &[VMPoolEntry]) -> PoolStatus {
    let total_vms = entries.len();
    let total_capacity = entries.iter().map(|entry| entry.capacity).sum();
    let active_sessions = entries
        .iter()
        .map(|entry| entry.active_sessions.len())
        .sum();
    let available_capacity = entries.iter().map(VMPoolEntry::available_capacity).sum();
    PoolStatus {
        total_vms,
        total_capacity,
        active_sessions,
        available_capacity,
    }
}

fn release_session_in_state(
    state_file: Option<PathBuf>,
    session_id: &str,
) -> Result<(), RemoteError> {
    let state_file = state_file.unwrap_or_else(default_state_file);
    let mut data = read_state_json(&state_file)?;
    let Some(pool) = data
        .get_mut("vm_pool")
        .and_then(|value| value.as_object_mut())
    else {
        return Ok(());
    };
    for entry in pool.values_mut() {
        let Some(active) = entry
            .get_mut("active_sessions")
            .and_then(|value| value.as_array_mut())
        else {
            continue;
        };
        active.retain(|value| value.as_str() != Some(session_id));
    }
    if let Some(parent) = state_file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| RemoteError::packaging(format!("Failed to create state dir: {e}")))?;
    }
    let content = serde_json::to_string_pretty(&data)
        .map_err(|e| RemoteError::packaging(format!("Failed to serialize state: {e}")))?;
    std::fs::write(&state_file, content)
        .map_err(|e| RemoteError::packaging(format!("Failed to write state: {e}")))?;
    Ok(())
}

fn read_state_json(state_file: &PathBuf) -> Result<serde_json::Value, RemoteError> {
    if !state_file.exists() {
        return Ok(serde_json::json!({"sessions": {}, "vm_pool": {}}));
    }
    let content = std::fs::read_to_string(state_file)
        .map_err(|e| RemoteError::packaging(format!("Failed to read state: {e}")))?;
    if content.trim().is_empty() {
        return Ok(serde_json::json!({"sessions": {}, "vm_pool": {}}));
    }
    serde_json::from_str(&content)
        .map_err(|e| RemoteError::packaging(format!("State file corrupt: {e}")))
}

fn default_state_file() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
        .join(".amplihack")
        .join("remote-state.json")
}
