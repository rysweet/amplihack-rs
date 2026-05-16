# Orchestration Infrastructure - Quick Start

## 5-Minute Guide

### Installation

The orchestration infrastructure is already installed at:

```
.claude/tools/amplihack/orchestration/
```

### Basic Usage

```rust
use std::path::PathBuf;
use amplihack_orchestration::{
    OrchestratorSession,
    run_parallel,
    run_sequential,
};
```

## Common Patterns

### Pattern 1: Parallel Multi-Agent Analysis

Run multiple agents in parallel:

```rust
// Create session
let session = OrchestratorSession::new("parallel-agents", None);

// Create agent processes
let agents = vec![
    session.create_process("Analyze security", "security"),
    session.create_process("Analyze performance", "performance"),
    session.create_process("Analyze maintainability", "maintainability"),
];

// Run in parallel
let results = run_parallel(agents, Some(3));

// Check results
for r in &results {
    if r.exit_code == 0 {
        println!("✓ {} completed in {:.1}s", r.process_id, r.duration);
    }
}
```

### Pattern 2: Sequential Pipeline

Run stages with output passing:

```rust
let session = OrchestratorSession::new("pipeline", None);

let stages = vec![
    session.create_process("Analyze code", "analyze"),
    session.create_process("Create plan", "plan"),
    session.create_process("Implement", "implement"),
];

let results = run_sequential(stages, true, false);
```

### Pattern 3: Retry with Fallback

Try different approaches:

```rust
use amplihack_orchestration::run_with_fallback;

let session = OrchestratorSession::new("retry", None);

let attempts = vec![
    session.create_process_with_timeout("Complex approach", "advanced", 300),
    session.create_process_with_timeout("Simple approach", "basic", 300),
];

let result = run_with_fallback(attempts, None);
```

### Pattern 4: Batched Processing

Process many items in controlled batches:

```rust
use amplihack_orchestration::run_batched;

let session = OrchestratorSession::new("batch", None);

// Create many processes
let processes: Vec<_> = (0..20)
    .map(|i| session.create_process(&format!("Process item {}", i), &format!("item-{}", i)))
    .collect();

// Run in batches of 5
let results = run_batched(processes, 5, false);
```

## Quick Reference

### OrchestratorSession

```rust
let session = OrchestratorSession::new_with_config(OrchestratorSessionConfig {
    pattern_name: "my-pattern".into(),      // Pattern identifier
    working_dir: Some(PathBuf::from("/project")),   // Optional: defaults to cwd
    base_log_dir: Some(PathBuf::from("/logs")),     // Optional: defaults to .claude/runtime/logs
    model: Some("claude-3-opus".into()),            // Optional: defaults to CLI default
});

// Factory method
let process = session.create_process_with_timeout(
    "Task description",       // prompt
    "task-001",               // process_id (optional: auto-generated if omitted)
    300,                      // timeout in seconds (optional)
);
```

### ClaudeProcess

```rust
use amplihack_orchestration::ClaudeProcess;

let process = ClaudeProcess::new(ClaudeProcessConfig {
    prompt: "Analyze this code".into(),
    process_id: "analyze-001".into(),
    working_dir: std::env::current_dir().unwrap(),
    log_dir: PathBuf::from(".logs"),
    model: Some("claude-3-sonnet".into()),  // Optional
    stream_output: true,                     // Optional: default true
    timeout: Some(300),                      // Optional: default None (no timeout)
});

let result = process.run()?;
```

### ProcessResult

```rust
// After running a process
let result = process.run()?;

// Check result
if result.exit_code == 0 {
    println!("Success! Output:\n{}", result.output);
} else {
    println!("Failed with code {}", result.exit_code);
    println!("Error: {}", result.stderr);
}

println!("Duration: {:.1}s", result.duration);
```

### Execution Helpers

```rust
use amplihack_orchestration::{
    run_parallel,
    run_sequential,
    run_with_fallback,
    run_batched,
};

// Parallel (independent tasks)
let results = run_parallel(processes, Some(3));

// Sequential (dependent tasks)
let results = run_sequential(
    processes,
    true,       // pass_output: pass output to next
    true,       // stop_on_failure: stop on first error
);

// Fallback (retry until success)
let result = run_with_fallback(processes, Some(Duration::from_secs(300)));

// Batched (controlled parallelism)
let results = run_batched(
    processes,
    5,          // batch_size
    true,       // pass_output: pass batch outputs
);
```

## Logging

All operations are logged automatically:

```bash
# Session log
.claude/runtime/logs/<session_id>/session.log

# Process logs
.claude/runtime/logs/<session_id>/<process_id>.log
```

## Exit Codes

- `0`: Success
- `-1`: Timeout or fatal error
- `> 0`: Claude CLI error

## Common Mistakes to Avoid

### ❌ Don't: Create processes without a session

```rust
// Hard to manage logs and state
let process = ClaudeProcess::new(config);
```

### ✅ Do: Use session factory

```rust
let session = OrchestratorSession::new("my-pattern", None);
let process = session.create_process(prompt, id);
```

### ❌ Don't: Run sequential when parallel is possible

```rust
// Slower for independent tasks
let results = run_sequential(independent_tasks, false, false);
```

### ✅ Do: Use parallel for independent tasks

```rust
let results = run_parallel(independent_tasks, None);
```

### ❌ Don't: Forget timeout for long-running tasks

```rust
let process = session.create_process(prompt, "id");  // No timeout!
```

### ✅ Do: Set reasonable timeouts

```rust
let process = session.create_process_with_timeout(prompt, "id", 300);
```

## Examples

See detailed examples in:

- `examples/usage.rs` - Runnable examples
- `README.md` - Complete documentation

## Troubleshooting

### Build Error

```bash
# Ensure the crate is in your workspace
cargo build -p amplihack-orchestration
```

### Process Hangs

- Check timeout is set
- Verify working directory exists
- Check logs in `~/.amplihack/.claude/runtime/logs/`

### Parallel Execution Issues

- Reduce `max_workers` if resource-constrained
- Check individual process logs
- Verify processes are independent

## Next Steps

1. **Read**: `README.md` for complete documentation
2. **Run**: `cargo run --example usage` for working examples
3. **Integrate**: Replace subprocess logic in existing code
4. **Extend**: Create patterns in `patterns/` directory

## Support

- Documentation: `README.md`
- Implementation: `IMPLEMENTATION_SUMMARY.md`
- Examples: `examples/usage.rs`
- Source: `claude_process.rs`, `execution.rs`, `session.rs`
