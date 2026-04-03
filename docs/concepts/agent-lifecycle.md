# Agent Lifecycle

The amplihack-rs agent system manages the complete lifecycle of AI agents —
from creation through execution to termination. This document describes the
architecture and state transitions that govern agent behavior.

## Overview

Every agent in amplihack-rs follows a unified lifecycle managed by the
`amplihack-agent-core` crate. The lifecycle is modeled as a state machine
with well-defined transitions and hooks at each phase.

```
Created → Initializing → Ready → Running → Completing → Done
                                    ↓           ↓
                                 Paused      Failed
                                    ↓
                                 Running
```

## States

| State          | Description                                       |
|----------------|---------------------------------------------------|
| `Created`      | Agent struct allocated, no resources acquired      |
| `Initializing` | Memory backend connecting, session state loading   |
| `Ready`        | All resources acquired, awaiting first input       |
| `Running`      | Actively processing input through the OODA loop    |
| `Paused`       | Temporarily suspended, resources held              |
| `Completing`   | Flushing memory, exporting state, cleanup in progress |
| `Done`         | Terminal state, all resources released             |
| `Failed`       | Terminal state, error captured for diagnostics     |

## Session Management

Each agent runs within a **session** — an isolated context that provides:

- **Session ID**: A UUID that partitions all memory operations
- **Working directory**: The filesystem scope for the agent
- **Memory partition**: Agent-specific memory namespace
- **Transcript log**: Full input/output history

```rust
use amplihack_agent_core::{AgentSession, SessionConfig};

let config = SessionConfig::builder()
    .session_id("abc-123")
    .working_dir("/path/to/project")
    .memory_backend(Backend::Sqlite)
    .build();

let session = AgentSession::new(config)?;
```

## OODA Loop

The core execution model follows the OODA (Observe–Orient–Decide–Act) loop,
implemented in the `AgenticLoop` engine:

1. **Observe**: Receive input (user message, tool result, event)
2. **Orient**: Recall relevant context from memory, classify intent
3. **Decide**: Choose an action (store knowledge, answer question, delegate)
4. **Act**: Execute the decision, write output, update memory

```rust
use amplihack_agent_core::{AgenticLoop, LoopConfig, LoopState};

let mut agent_loop = AgenticLoop::new(LoopConfig {
    max_iterations: 10,
    model: "claude-opus-4-6".into(),
    memory: session.memory_handle(),
    ..Default::default()
});

let result = agent_loop.process("What is the capital of France?")?;
println!("Answer: {}", result.output);
```

## Agent Configuration

Agents are configured through `AgentConfig`, which controls behavior,
resource limits, and integration points:

```rust
use amplihack_agent_core::AgentConfig;

let config = AgentConfig {
    name: "my-agent".into(),
    model: "claude-sonnet-4-5".into(),
    max_turns: 100,
    memory_backend: Backend::Sqlite,
    memory_topology: Topology::Single,
    timeout_secs: 300,
    working_dir: Some("/path/to/project".into()),
    capabilities: vec![Capability::CodeExecution, Capability::FileAccess],
    ..Default::default()
};
```

### Configuration Sources (Priority Order)

1. Explicit `AgentConfig` fields (highest priority)
2. Environment variables (`AMPLIHACK_AGENT_MODEL`, `AMPLIHACK_MEMORY_BACKEND`, etc.)
3. Project-level `.amplihack/config.toml`
4. User-level `~/.amplihack/config.toml`
5. Compiled defaults (lowest priority)

## Memory Integration

Every agent has access to the memory subsystem through its session. The
memory facade provides two primary operations:

```rust
// Store a fact
session.memory().remember("The deployment uses Kubernetes 1.28")?;

// Recall relevant facts
let facts = session.memory().recall("deployment infrastructure")?;
for fact in &facts {
    println!("{}: {} (relevance: {:.2})", fact.memory_type, fact.content, fact.relevance);
}
```

See [Memory Backend Architecture](./memory-backend-architecture.md) for
details on backend selection and topology modes.

## Subprocess Isolation

When agents need to execute code or run tools, they do so through
subprocess isolation. The `AgentSubprocess` wrapper provides:

- **Timeout enforcement**: Hard kill after configurable deadline
- **Output capture**: Stdout/stderr collected with size limits
- **Signal forwarding**: SIGINT/SIGTERM propagated to child
- **Resource limits**: Optional cgroup integration for memory/CPU

```rust
use amplihack_agent_core::AgentSubprocess;

let result = AgentSubprocess::new("python3")
    .arg("evaluate.py")
    .timeout(Duration::from_secs(60))
    .capture_output(true)
    .run()?;
```

## Related

- [Memory Backend Architecture](./memory-backend-architecture.md) — How memory backends are selected and configured
- [Agent Binary Routing](./agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` routes to the correct tool
- [Recipe Execution Flow](./recipe-execution-flow.md) — How recipes orchestrate agents
