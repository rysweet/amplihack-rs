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
    pub timeout_secs: u64,
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
```

### AgentSession

```rust
pub struct AgentSession {
    /* private fields */
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
