# Orchestration Infrastructure

Complete orchestration infrastructure for managing multiple Claude CLI processes with different execution patterns.

## Overview

This module provides the core building blocks for orchestrating Claude processes:

- **ClaudeProcess**: Manages a single subprocess with output capture, timeout, and logging
- **ProcessResult**: Structured result with exit code, output, duration, and process ID
- **OrchestratorSession**: Session management with logging and process factory
- **Execution Helpers**: Functions for parallel, sequential, fallback, and batched execution

## Module Structure

```
crates/amplihack-orchestration/
├── src/
│   ├── lib.rs              # Public exports
│   ├── claude_process.rs   # Core process management
│   ├── execution.rs        # Execution strategies
│   ├── session.rs          # Session management
│   └── patterns/
│       └── mod.rs          # Reusable patterns (future)
├── examples/
│   └── usage.rs            # Comprehensive usage examples
└── docs/
    └── README.md           # This file
```

## Core Components

### ClaudeProcess

Manages a single Claude CLI subprocess:

```rust
use amplihack_orchestration::ClaudeProcess;

let process = ClaudeProcess::new(ClaudeProcessConfig {
    prompt: "Analyze this code".into(),
    process_id: "security-analysis".into(),
    working_dir: PathBuf::from("/project"),
    log_dir: PathBuf::from("/logs"),
    model: Some("claude-3-opus".into()),
    stream_output: true,
    timeout: Some(300),
});

let result = process.run()?;
println!("Exit code: {}", result.exit_code);
println!("Duration: {}s", result.duration);
println!("Output: {}", result.output);
```

**Features**:

- PTY-based stdin to prevent blocking
- Real-time output streaming
- Thread-based output capture
- Timeout support with graceful termination
- Comprehensive logging

### ProcessResult

Structured result from process execution:

```rust
pub struct ProcessResult {
    pub exit_code: i32,      // 0=success, -1=timeout, other=error
    pub output: String,      // Combined stdout
    pub stderr: String,      // Stderr output
    pub duration: f64,       // Execution time in seconds
    pub process_id: String,  // Process identifier
}
```

### OrchestratorSession

Session management with logging:

```rust
use amplihack_orchestration::OrchestratorSession;

let session = OrchestratorSession::new(
    "parallel-analysis",
    PathBuf::from("/project"),
);

// Factory method creates configured processes
let p1 = session.create_process("Analyze security", "security");
let p2 = session.create_process("Analyze performance", "performance");

session.log("Starting analysis");
```

## Execution Strategies

### Parallel Execution

Run multiple processes concurrently:

```rust
use amplihack_orchestration::run_parallel;

let results = run_parallel(processes, Some(3));

let successful: Vec<_> = results.iter().filter(|r| r.exit_code == 0).collect();
println!("{}/{} succeeded", successful.len(), results.len());
```

**Use cases**:

- Multi-agent analysis
- Independent tasks
- Batch processing

### Sequential Execution

Run processes one at a time:

```rust
use amplihack_orchestration::run_sequential;

// With output passing
let results = run_sequential(
    processes,
    true,      // pass_output: pass output to next process
    true,      // stop_on_failure: stop on first error
);
```

**Use cases**:

- Pipeline stages
- Dependent tasks
- Iterative refinement

### Fallback Strategy

Try processes until one succeeds:

```rust
use amplihack_orchestration::run_with_fallback;

let result = run_with_fallback(processes, Some(Duration::from_secs(300)));

if result.exit_code == 0 {
    println!("Succeeded with: {}", result.process_id);
}
```

**Use cases**:

- Multiple approaches
- Different models
- Retry with degraded capabilities

### Batched Execution

Run processes in parallel batches:

```rust
use amplihack_orchestration::run_batched;

let results = run_batched(
    processes,
    3,         // batch_size
    true,      // pass_output: pass batch outputs to next batch
);
```

**Use cases**:

- Resource-limited parallelism
- Progressive processing
- Batch dependencies

## Usage Examples

### Multi-Agent Parallel Analysis

```rust
use std::path::PathBuf;
use amplihack_orchestration::{OrchestratorSession, run_parallel};

// Create session
let session = OrchestratorSession::new("multi-agent", None);

// Create agent processes
let agents = vec![
    session.create_process("Analyze security", "security"),
    session.create_process("Analyze performance", "performance"),
    session.create_process("Analyze maintainability", "maintainability"),
];

// Run in parallel
let results = run_parallel(agents, Some(3));

// Process results
for result in &results {
    if result.exit_code == 0 {
        println!("✓ {}: {:.1}s", result.process_id, result.duration);
    } else {
        println!("✗ {}: FAILED", result.process_id);
    }
}
```

### Sequential Pipeline

```rust
let session = OrchestratorSession::new("pipeline", None);

let stages = vec![
    session.create_process("Analyze codebase", "analyze"),
    session.create_process("Create improvement plan", "plan"),
    session.create_process("Implement improvements", "implement"),
];

let results = run_sequential(stages, true, true);
```

### Adaptive Fallback

```rust
let session = OrchestratorSession::new("adaptive", None);

let strategies = vec![
    session.create_process_with_timeout("Advanced analysis", "advanced", 300),
    session.create_process_with_timeout("Standard analysis", "standard", 300),
    session.create_process_with_timeout("Basic analysis", "basic", 300),
];

let result = run_with_fallback(strategies, Some(Duration::from_secs(300)));
```

## Design Principles

### Ruthless Simplicity

- Direct implementation, no over-engineering
- Reuse proven patterns from auto_mode.rs
- Clear contracts and boundaries

### Modular Design (Bricks & Studs)

- **ClaudeProcess** = Brick (self-contained subprocess management)
- **Execution helpers** = Studs (clear coordination interfaces)
- **OrchestratorSession** = Context (shared session state)

### Zero-BS Implementation

- No stubs or placeholders
- Every function works or doesn't exist
- Comprehensive error handling
- Real logging and output capture

## Regeneration

This module can be regenerated from:

1. This README (specification)
2. auto_mode.rs (proven subprocess patterns)
3. Project philosophy (PHILOSOPHY.md)

Key extraction points from auto_mode.rs:

- Subprocess mechanics with PTY
- PTY setup and Command::new()
- Thread-based output capture
- PTY stdin feeding

## Future Extensions

Potential additions (not implemented yet):

1. **Pattern Library**: Pre-built orchestration patterns
2. **Result Aggregation**: Structured result combination
3. **Progress Tracking**: Real-time progress monitoring
4. **Resource Management**: CPU/memory limits per process
5. **Retry Logic**: Configurable retry strategies
6. **Output Filtering**: Selective output capture

## Testing

To test the infrastructure:

```bash
# Run tests
cargo test -p amplihack-orchestration

# Run examples
cargo run --example usage

# Check logs
ls -la .claude/runtime/logs/

# Verify specific session
cat .claude/runtime/logs/<session_id>/session.log
```

## Integration

The orchestration infrastructure is designed to be integrated with:

- **Auto mode**: Replace inline subprocess logic
- **Workflow engine**: Orchestrate workflow steps
- **Agent system**: Coordinate multiple agents
- **CI/CD**: Parallel test execution

## Contract

### ClaudeProcess Contract

**Inputs**:

- prompt: String (required)
- process_id: String (required)
- working_dir: PathBuf (required)
- log_dir: PathBuf (required)
- model: Option<String>
- stream_output: bool = true
- timeout: Option<u64> = None

**Outputs**:

- ProcessResult with exit_code, output, stderr, duration, process_id

**Guarantees**:

- Logs all operations
- Cleans up resources (PTY, threads)
- Handles timeout gracefully
- Never blocks indefinitely
- Captures all output

### Execution Helpers Contract

**run_parallel**:

- Input: Vec<ClaudeProcess>, optional max_workers: Option<usize>
- Output: Vec<ProcessResult> in completion order
- Guarantees: All processes execute, errors converted to failed results

**run_sequential**:

- Input: Vec<ClaudeProcess>, optional pass_output: bool, stop_on_failure: bool
- Output: Vec<ProcessResult> in execution order
- Guarantees: Order preserved, output passing works, stops on failure if requested

**run_with_fallback**:

- Input: Vec<ClaudeProcess>, optional timeout: Option<Duration>
- Output: Single ProcessResult (first success or last failure)
- Guarantees: Tries all until success, applies timeout to each

**run_batched**:

- Input: Vec<ClaudeProcess>, batch_size: usize, optional pass_output: bool
- Output: Vec<ProcessResult> in batch completion order
- Guarantees: Batches process in order, batch outputs can pass to next batch

## License

Part of the Microsoft Hackathon 2025 Agentic Coding Framework.
