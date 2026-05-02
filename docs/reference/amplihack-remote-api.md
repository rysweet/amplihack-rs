# amplihack-remote API

The `amplihack-remote` crate is the native Rust API behind
`amplihack remote`. It owns remote execution behavior, VM pooling, detached
session state, output capture, and result integration. CLI code in
`crates/amplihack-cli` only parses arguments, calls this crate, and renders the
returned values.

## Contents

- [Design contract](#design-contract)
- [Target implementation contract](#target-implementation-contract)
- [High-level command API](#high-level-command-api)
- [Synchronous execution](#synchronous-execution)
- [Detached sessions](#detached-sessions)
- [VM pooling](#vm-pooling)
- [State management](#state-management)
- [Errors](#errors)
- [Testing seams](#testing-seams)

## Design contract

`amplihack-remote` exposes public functions for the six supported remote
commands:

- `exec`
- `list`
- `start`
- `output`
- `kill`
- `status`

The crate also keeps lower-level public building blocks for consumers that need
direct control over packaging, provisioning, execution, integration, or session
state.

The Python remote implementation is not part of the runtime. The Rust API is the
single supported implementation surface.

## Target implementation contract

This page is the issue #536 target contract. Existing lower-level modules remain
public during the migration, but `amplihack-cli` should call only the
command-shaped API once the `api` module lands.

The following behaviors are part of the build target:

- `exec` and `start_sessions` both require agent credentials. CLI code reads
  `ANTHROPIC_API_KEY`, passes it into the API, and validation fails before
  repository packaging, VM provisioning, or VM pool allocation begins.
- Detached sessions use `NODE_OPTIONS=--max-old-space-size=32768`. The persisted
  session state schema includes `memory_mb`, and sessions launched through
  `start_sessions` record `memory_mb: 32768`.

## High-level command API

The `api` module provides command-shaped request and response types.

```rust
use std::path::PathBuf;

use amplihack_remote::{
    CommandMode, ExecOptions, ListOptions, OutputOptions, RemoteError,
    StartOptions, VMOptions, VMSize,
};

async fn run_remote(repo: PathBuf, api_key: String) -> Result<(), RemoteError> {
    let result = amplihack_remote::exec(ExecOptions {
        repo_path: repo,
        command: CommandMode::Auto,
        prompt: "implement remote list JSON output".to_string(),
        max_turns: 10,
        vm_options: VMOptions::default(),
        timeout_minutes: 120,
        skip_secret_scan: false,
        api_key,
    })
    .await?;

    println!("remote exit code: {}", result.exit_code);
    Ok(())
}
```

### `CommandMode`

```rust
pub enum CommandMode {
    Auto,
    Ultrathink,
    Analyze,
    Fix,
}
```

`CommandMode` implements string parsing and display using the CLI values
`auto`, `ultrathink`, `analyze`, and `fix`.

### Validation

High-level options validate the same rules as the CLI:

| Field | Rule |
| ----- | ---- |
| `prompt` | Must not be empty after trimming whitespace. |
| `command` | Must be one of `auto`, `ultrathink`, `analyze`, or `fix`. |
| `max_turns` for `exec` | Must be `1..=50`. |
| `timeout_minutes` for `exec` | Must be `5..=480`. |
| `api_key` for `exec` and `start` | Must not be empty. |
| `start` prompts | At least one non-empty prompt is required. |
| `output` session ID | Must exist in the state store. |
| `kill` session ID | Must exist in the state store. |

Validation failures return `RemoteError` and do not partially update state.

## Synchronous execution

Use `exec` for the blocking one-shot workflow.

```rust
pub async fn exec(options: ExecOptions) -> Result<WorkflowResult, RemoteError>;
```

`exec` performs:

1. Repository, option, and credential validation.
2. Context packaging and secret scanning.
3. VM provisioning or reuse through azlin.
4. Context transfer.
5. Remote command execution.
6. Log and git-state retrieval.
7. Result integration and VM cleanup.

### `ExecOptions`

```rust
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
```

### `WorkflowResult`

```rust
pub struct WorkflowResult {
    pub exit_code: i32,
    pub summary: Option<IntegrationSummary>,
    pub vm_name: Option<String>,
}
```

`execute_remote_workflow` remains available as a compatibility wrapper around
`exec` for existing Rust callers.

## Detached sessions

Use detached session APIs for `remote start`, `list`, `output`, `kill`, and
`status`.

### Start sessions

```rust
pub async fn start_sessions(options: StartOptions) -> Result<StartResult, RemoteError>;
```

```rust
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

pub struct StartResult {
    pub sessions: Vec<Session>,
}
```

`start_sessions` validates all prompts and the API key before side effects begin.
It then packages context separately for each prompt, allocates VM pool capacity,
transfers context, starts a tmux session, and marks each successful session
`running`.

### List sessions

```rust
pub fn list_sessions(options: ListOptions) -> Result<Vec<Session>, RemoteError>;
```

```rust
pub struct ListOptions {
    pub status: Option<SessionStatus>,
    pub state_file: Option<PathBuf>,
}
```

### Capture output

```rust
pub async fn capture_output(options: OutputOptions) -> Result<SessionOutput, RemoteError>;
```

```rust
pub struct OutputOptions {
    pub session_id: String,
    pub lines: u32,
    pub state_file: Option<PathBuf>,
}

pub struct SessionOutput {
    pub session: Session,
    pub output: String,
}
```

Unknown session IDs return `RemoteError::SessionNotFound` rather than an empty
string.

### Kill sessions

```rust
pub async fn kill_session(options: KillOptions) -> Result<KillResult, RemoteError>;
```

```rust
pub struct KillOptions {
    pub session_id: String,
    pub force: bool,
    pub state_file: Option<PathBuf>,
}

pub struct KillResult {
    pub session: Session,
    pub remote_kill_attempted: bool,
    pub remote_kill_succeeded: bool,
    pub capacity_released: bool,
}
```

Without `force`, remote tmux kill failures return an error and leave local state
unchanged. With `force`, state is updated even when the VM is unreachable.

### Status

```rust
pub fn status(options: StatusOptions) -> Result<RemoteStatus, RemoteError>;
```

```rust
pub struct StatusOptions {
    pub state_file: Option<PathBuf>,
}

pub struct RemoteStatus {
    pub pool: PoolStatus,
    pub sessions: SessionCounts,
    pub total_sessions: usize,
}

pub struct SessionCounts {
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub killed: usize,
    pub pending: usize,
}
```

## VM pooling

`VMSize` maps detached session tiers to Azure SKUs and capacity.

```rust
pub enum VMSize {
    S,
    M,
    L,
    XL,
}
```

| Variant | Azure SKU | Capacity |
| ------- | --------- | -------- |
| `S` | `Standard_D8s_v3` | 1 |
| `M` | `Standard_E8s_v5` | 2 |
| `L` | `Standard_E16s_v5` | 4 |
| `XL` | `Standard_E32s_v5` | 8 |

```rust
let size = VMSize::L;
assert_eq!(size.capacity(), 4);
assert_eq!(size.azure_size(), "Standard_E16s_v5");
```

## State management

`SessionManager` owns session lifecycle state.

```rust
let mut manager = SessionManager::new(None)?;
let session = manager.create_session(
    "amplihack-azureuser-20260502",
    "implement remote status JSON",
    Some("auto"),
    Some(10),
    Some(32_768),
)?;

manager.start_session(&session.session_id)?;
```

Detached sessions created by `start_sessions` store `Some(32_768)` for
`memory_mb`. This value is part of the persisted state contract and mirrors the
remote tmux environment's `NODE_OPTIONS=--max-old-space-size=32768` setting.

Session state values are serialized as lowercase strings:

```rust
pub enum SessionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
}
```

All state writes use the `state_lock` module so concurrent processes update the
same state file safely.

## Errors

Public APIs return `RemoteError`.

```rust
pub enum RemoteError {
    Packaging(ErrorContext),
    Provisioning(ErrorContext),
    Transfer(ErrorContext),
    Execution(ErrorContext),
    Integration(ErrorContext),
    Cleanup(ErrorContext),
    SessionNotFound { session_id: String },
    Validation(ErrorContext),
}
```

Each error carries a user-facing message and optional context. CLI handlers map
errors to process exit codes:

| Error | Exit code |
| ----- | --------- |
| `SessionNotFound` | `3` |
| Validation, packaging, provisioning, transfer, execution, integration, cleanup | `1` |
| CLI parser errors before API dispatch | `2` |

## Testing seams

Remote infrastructure calls are behind traits so unit tests do not require live
Azure resources:

```rust
#[async_trait::async_trait]
pub trait RemoteCommandRunner {
    async fn azlin(&self, args: &[String]) -> Result<CommandOutput, RemoteError>;
    async fn connect(&self, vm_name: &str, command: &str) -> Result<CommandOutput, RemoteError>;
    async fn transfer(&self, source: &Path, vm_name: &str, destination: &Path)
        -> Result<(), RemoteError>;
}
```

The production runner shells out to `azlin`. Tests use an in-memory runner to
assert command construction, state transitions, output parsing, force-kill
behavior, and JSON response shape.
