# amplihack-agent-core

API reference for the `amplihack-agent-core` crate — agent lifecycle management,
session isolation, and OODA loop execution.

## Crate Overview

`amplihack-agent-core` provides the foundational types and execution engine
for all amplihack agents. It manages agent lifecycle state transitions,
session isolation, memory integration, and the OODA loop that drives
agent reasoning.

**Workspace dependency**: `amplihack-agent-core = { path = "crates/amplihack-agent-core" }`

## Modules

| Module       | Description                                          |
|--------------|------------------------------------------------------|
| `agent`      | `Agent` struct, `AgentConfig`, lifecycle state machine |
| `session`    | `AgentSession`, `SessionConfig`, session isolation   |
| `ooda`       | `AgenticLoop`, `LoopState`, `LoopConfig`             |
| `subprocess` | `AgentSubprocess` — isolated process execution       |
| `error`      | `AgentError` enum with `thiserror` derives           |
| `models`     | `TaskResult`, `Capability`, `AgentStatus`            |

## Core Types

### AgentConfig

```rust
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub max_turns: u32,
    pub memory_backend: Backend,
    pub memory_topology: Topology,
    pub timeout_secs: Option<u64>, // None = no limit (#2022)
    pub working_dir: Option<PathBuf>,
    pub capabilities: Vec<Capability>,
}
```

Configuration is resolved from (highest to lowest priority):
1. Explicit struct fields
2. Environment variables (`AMPLIHACK_AGENT_MODEL`, `AMPLIHACK_MEMORY_BACKEND`)
3. Project config `.amplihack/config.toml`
4. User config `~/.amplihack/config.toml`
5. Compiled defaults

### Agent

```rust
pub struct Agent {
    config: AgentConfig,
    session: AgentSession,
    status: AgentStatus,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Result<Self, AgentError>;
    pub fn status(&self) -> AgentStatus;
    pub fn session(&self) -> &AgentSession;
    pub fn process(&mut self, input: &str) -> Result<TaskResult, AgentError>;
    pub fn shutdown(self) -> Result<(), AgentError>;
}
```

### AgentStatus

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Created,
    Initializing,
    Ready,
    Running,
    Paused,
    Completing,
    Done,
    Failed,
}
```

### SessionConfig

```rust
pub struct SessionConfig {
    pub session_id: String,
    pub working_dir: PathBuf,
    pub memory_backend: Backend,
    pub memory_topology: Topology,
    pub transcript_enabled: bool,
}

impl SessionConfig {
    pub fn builder() -> SessionConfigBuilder;
}

/// Builder for `SessionConfig`.
pub struct SessionConfigBuilder { /* private */ }

impl SessionConfigBuilder {
    pub fn session_id(self, id: &str) -> Self;
    pub fn working_dir(self, dir: impl Into<PathBuf>) -> Self;
    pub fn memory_backend(self, backend: Backend) -> Self;
    pub fn memory_topology(self, topology: Topology) -> Self;
    pub fn transcript_enabled(self, enabled: bool) -> Self;
    pub fn build(self) -> SessionConfig;
}
```

### AgentSession

```rust
pub struct AgentSession {
    /* private fields */
}

/// A cloneable handle to the session's memory subsystem.
pub struct MemoryHandle {
    /* private — wraps Arc<Memory> */
}

impl MemoryHandle {
    pub fn remember(&self, content: &str) -> Result<String, MemoryError>;
    pub fn recall(&self, query: &str) -> Result<Vec<RecallResult>, MemoryError>;
}

impl AgentSession {
    pub fn new(config: SessionConfig) -> Result<Self, AgentError>;
    pub fn id(&self) -> &str;
    pub fn working_dir(&self) -> &Path;
    pub fn memory_handle(&self) -> MemoryHandle;
    pub fn transcript(&self) -> &[TranscriptEntry];
    pub fn close(self) -> Result<(), AgentError>;
}
```

### AgenticLoop

```rust
pub struct LoopConfig {
    pub max_iterations: u32,
    pub model: String,
    pub memory: MemoryHandle,
    pub orient_search_limit: usize,
    pub timeout: Duration,
}

pub struct AgenticLoop {
    /* private fields */
}

impl AgenticLoop {
    pub fn new(config: LoopConfig) -> Self;
    pub fn process(&mut self, input: &str) -> Result<TaskResult, AgentError>;
    pub fn observe(&mut self, input: &str) -> Result<(), AgentError>;
    pub fn orient(&mut self) -> Result<OrientResult, AgentError>;
    pub fn decide(&mut self) -> Result<Decision, AgentError>;
    pub fn act(&mut self) -> Result<ActionResult, AgentError>;
}
```

### LoopState

```rust
pub struct LoopState {
    pub perception: String,
    pub reasoning: String,
    pub action: Action,
    pub learning: String,
    pub outcome: Value,
    pub iteration: u32,
}
```

### TaskResult

```rust
pub struct TaskResult {
    pub output: String,
    pub confidence: f64,
    pub metadata: HashMap<String, Value>,
    pub memory_updates: Vec<MemoryEntry>,
}
```

### Capability

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    CodeExecution,
    FileAccess,
    WebSearch,
    MemoryAccess,
    ToolUse,
    Delegation,
}
```

### AgentSubprocess

```rust
pub struct AgentSubprocess {
    /* private fields */
}

impl AgentSubprocess {
    pub fn new(binary: &str) -> Self;
    pub fn arg(self, arg: &str) -> Self;
    pub fn args(self, args: &[&str]) -> Self;
    pub fn timeout(self, duration: Duration) -> Self;
    pub fn capture_output(self, capture: bool) -> Self;
    pub fn working_dir(self, dir: &Path) -> Self;
    pub fn env(self, key: &str, value: &str) -> Self;
    pub fn run(&self) -> Result<SubprocessResult, AgentError>;
}

pub struct SubprocessResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}
```

### AgentError

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("session error: {0}")]
    Session(String),
    #[error("memory error: {0}")]
    Memory(#[from] MemoryError),
    #[error("subprocess failed: {0}")]
    Subprocess(String),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("invalid state transition: {from:?} → {to:?}")]
    InvalidTransition { from: AgentStatus, to: AgentStatus },
    #[error("configuration error: {0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

## Timeout fields

No agent-core type has a default timeout (#2022). Each timeout field is an
`Option` that defaults to `None`, meaning no limit. A value exists only when
config or a builder sets one. See
[No default time limits on work](../concepts/no-default-time-limits.md).

| Type | Field | Type | Default | Set by | Read back with |
|---|---|---|---|---|---|
| `models::AgentConfig` | `timeout_secs` | `Option<u64>` | `None` | `with_timeout(secs)` or `timeout_secs` in config | the field |
| `models::TaskSpec` | `timeout_secs` | `Option<u64>` | `None` | `with_timeout(secs)` or `timeout_secs` in JSON | `timeout() -> Option<Duration>` |
| `sdk_adapters::SdkAdapterConfig` | `timeout_secs` | `Option<f64>` | `None` | `with_timeout(secs)` or `timeout_secs` in config | the field |
| `sdk_adapters::CopilotAdapter` | (private) | `Option<f64>` | `None` | `SdkAdapterConfig.timeout_secs` or `with_timeout(secs)` | `timeout() -> Option<f64>` |
| `sdk_adapters::CopilotCliClient` | (private) | `Option<Duration>` | `None` | `with_timeout(Duration)` | not exposed |

Rules:

- **Serde.** All three config fields use
  `#[serde(default, skip_serializing_if = "Option::is_none")]`. JSON or TOML
  without the key loads as `None`; `{"timeout_secs": 300}` loads as
  `Some(300)`. A `None` field is left out of serialised output.
- **No sentinels.** `Some(0)` is stored as given and never means "unlimited".
- **`CopilotAdapter`** raises a set value below 1 second to 1 second and
  keeps larger values unchanged; there is no upper clamp (#867). It records
  the value and logs it as `timeout = None` or `timeout = Some(..)`. It does
  not pass the value to the client, so it is not enforced.
- **`CopilotCliClient`** with a timeout set uses it as the idle window of the
  [idle watchdog](./idle-watchdog.md). With `None` the call has no limit.
- **`AgentConfig` and `TaskSpec`** values are recorded for callers and are
  not enforced by agent-core. A caller that enforces one should convert with
  `Duration::try_from_secs_f64` (for `f64`) or `Duration::from_secs`, and
  treat `None` as no limit.

```rust
use std::time::Duration;
use amplihack_agent_core::models::TaskSpec;
use amplihack_agent_core::sdk_adapters::types::{SdkAdapterConfig, SdkType};

let task = TaskSpec::new("Summarise the open review threads on PR 2050");
assert_eq!(task.timeout(), None); // no limit

let task = task.with_timeout(30);
assert_eq!(task.timeout(), Some(Duration::from_secs(30)));

let cfg = SdkAdapterConfig::new("reviewer", SdkType::Copilot);
assert_eq!(cfg.timeout_secs, None);
assert_eq!(cfg.with_timeout(60.0).timeout_secs, Some(60.0));
```

## Feature Flags

None. This crate has no optional features.

## Dependencies

| Crate              | Purpose                       |
|--------------------|-------------------------------|
| `amplihack-memory` | Memory backend integration    |
| `amplihack-types`  | Shared IPC types              |
| `serde`            | Serialization                 |
| `serde_json`       | JSON handling                 |
| `thiserror`        | Error derive macros           |
| `tracing`          | Structured logging            |
| `chrono`           | Timestamps                    |
