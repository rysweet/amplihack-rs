# .claude/tools/ Directory

**Comprehensive toolkit for Claude Code integration, hooks, session management, and AI-powered development workflows.**

This directory contains the core infrastructure for the Amplihack framework's integration with Claude Code, providing hooks, builders, orchestration, memory systems, and session management.

## Table of Contents

- [Directory Structure](#directory-structure)
- [Hook System](#hook-system)
- [Builders](#builders)
- [Orchestration](#orchestration)
- [Memory System](#memory-system)
- [Session Management](#session-management)
- [Reflection System](#reflection-system)
- [Utilities](#utilities)
- [Quick Start](#quick-start)
- [Integration Examples](#integration-examples)

## Directory Structure

```
.claude/tools/
├── amplihack/                  # Main amplihack framework tools
│   ├── builders/               # Transcript and documentation builders
│   │   ├── claude_transcript_builder.rs    # Session transcripts
│   │   ├── codex_transcripts_builder.rs    # Codex-optimized exports
│   │   └── export_on_compact_integration.rs # Compaction integration
│   ├── orchestration/          # Multi-process orchestration
│   │   ├── claude_process.rs   # Single process execution
│   │   ├── execution.rs        # Parallel/sequential/fallback execution
│   │   ├── session.rs          # Session coordination
│   │   └── patterns/           # Fault tolerance patterns
│   │       ├── n_version.rs    # N-version programming
│   │       ├── debate.rs       # Multi-agent debate
│   │       ├── cascade.rs      # Fallback cascade
│   │       └── expert_panel.rs # Expert consensus
│   ├── memory/                 # Agent memory system
│   │   ├── interface.rs        # Clean API for memory operations
│   │   ├── core.rs             # SQLite-based backend
│   │   ├── context_preservation.rs # Context management
│   │   └── examples/           # Usage examples
│   ├── reflection/             # AI-powered reflection system
│   │   ├── reflection.rs       # Session analysis and improvement
│   │   ├── semantic_duplicate_detector.rs # Issue deduplication
│   │   ├── contextual_error_analyzer.rs   # Error pattern analysis
│   │   ├── security.rs         # Content sanitization
│   │   └── display.rs          # User-facing output
│   ├── session/                # Session lifecycle management
│   │   ├── session_toolkit.rs  # Unified session interface
│   │   ├── claude_session.rs   # Core session implementation
│   │   ├── session_manager.rs  # Multi-session coordination
│   │   ├── toolkit_logger.rs   # Session logging
│   │   └── file_utils.rs       # Safe file operations
│   ├── context_preservation.rs # Context extraction and export
│   ├── xpia_defense.rs         # Cross-Process Injection Attack defense
│   └── paths.rs                # Path utilities
├── ci_status.rs                # CI/CD status checking
├── ci_workflow.rs              # CI diagnostic workflow
├── precommit_workflow.rs       # Pre-commit diagnostic workflow
├── github_issue.rs             # GitHub issue creation
├── improvement_validator.rs    # Improvement validation
└── test-utilities/             # Testing utilities
```

## Hook System

The hook system integrates with Claude Code's lifecycle events to provide session management, preference enforcement, and context preservation.

### Hook Lifecycle

```
1. session_start        → Initialize session, inject context
2. user_prompt_submit   → Inject user preferences (every turn)
3. [tool operations]    → Normal Claude Code operations
4. post_tool_use        → Track tool usage metrics
5. pre_compact          → Export conversation before compaction
6. stop                 → Check lock flag, trigger reflection
```

### Available Hooks

#### `session_start`

**Purpose**: Initialize session with context and preferences

**What it does**:

- Injects project context (PHILOSOPHY.md, DISCOVERIES.md)
- Loads and enforces USER_PREFERENCES.md (MANDATORY)
- Captures original request for context preservation
- Stages UVX framework if deployed via uvx
- Provides workflow information

**Returns**: `{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "..."}}`

**Example**:

```rust
// Automatically triggered by Claude Code on session start
// Injects context visible to Claude in the conversation
```

#### `user_prompt_submit`

**Purpose**: Inject user preferences on every user message

**What it does**:

- Reads USER_PREFERENCES.md on each user prompt
- Extracts key preferences (communication style, verbosity, etc.)
- Caches preferences for performance (invalidates on file change)
- Injects concise preference context to enforce behavior

**Returns**: `{"additionalContext": "🎯 ACTIVE USER PREFERENCES (MANDATORY): ..."}`

**Example**:

```rust
// Automatically triggered on every user message
// Ensures preferences persist across conversation turns
```

#### `post_tool_use`

**Purpose**: Track tool usage and collect metrics

**What it does**:

- Logs every tool invocation
- Saves structured metrics (tool name, duration)
- Categorizes tools (bash, file operations, search)
- Detects and logs tool errors

**Returns**: `{}` or `{"metadata": {"warning": "..."}}`

**Example**:

```rust
// Automatically triggered after each tool use
// Metrics saved to .claude/runtime/metrics/
```

#### `pre_compact`

**Purpose**: Export conversation before context compaction

**What it does**:

- Receives full conversation history from Claude Code
- Exports to CONVERSATION_TRANSCRIPT.md
- Preserves original request if available
- Creates timestamped backup copies
- Saves compaction event metadata

**Returns**: `{"status": "success", "message": "...", "transcript_path": "..."}`

**Example**:

```rust
// Automatically triggered before Claude Code compacts context
// Ensures no conversation history is lost
```

#### `stop`

**Purpose**: Control stop behavior with lock flag

**What it does**:

- Checks for lock flag (`~/.amplihack/.claude/runtime/locks/.lock_active`)
- Blocks stop if lock is active (continuous work mode)
- Triggers reflection analysis if enabled
- Creates reflection pending marker

**Returns**:

- `{"decision": "approve"}` - Allow stop
- `{"decision": "block", "reason": "..."}` - Continue working

**Example**:

```rust
// Automatically triggered when Claude tries to stop
// Lock flag enables continuous multi-turn work
```

### Base Hook Processor

> **Note (rysweet/amplihack-rs#285):** the bundled `amplihack/hooks/`
> tree (including `hook_processor`) was removed. Hook orchestration in
> amplihack-rs is implemented in the Rust `amplihack-hooks` crate; consult
> that crate for the current API.

## Builders

Builders create structured documentation and exports from session data.

### Claude Transcript Builder

**File**: `crates/amplihack-builders/src/claude_transcript_builder.rs`

**Purpose**: Build comprehensive session transcripts for documentation and knowledge extraction

**Usage**:

```rust
use amplihack_builders::ClaudeTranscriptBuilder;

// Create builder
let builder = ClaudeTranscriptBuilder::new("20250105_143022");

// Build transcript
let transcript_path = builder.build_session_transcript(
    &[
        Message { role: "user", content: "Hello", timestamp: "2025-01-05T14:30:22" },
        Message { role: "assistant", content: "Hi!", timestamp: "2025-01-05T14:30:23" },
    ],
    &Metadata { project: "amplihack".into() },
)?;

// Build session summary
let summary = builder.build_session_summary(&messages, &metadata)?;
println!("Total words: {}", summary.total_words);

// Export for codex
let codex_path = builder.export_for_codex(&messages, &metadata)?;
```

**Outputs**:

- `CONVERSATION_TRANSCRIPT.md` - Human-readable markdown transcript
- `conversation_transcript.json` - Machine-readable JSON format
- `session_summary.json` - Statistical summary
- `codex_export.json` - Knowledge extraction optimized format

### Codex Transcripts Builder

**File**: `crates/amplihack-builders/src/codex_transcripts_builder.rs`

**Purpose**: Create codex-optimized exports for knowledge systems

**Features**:

- Pattern detection (tool usage, error-fix cycles)
- Decision extraction
- Knowledge artifact identification
- Conversation flow analysis

## Orchestration

Multi-process orchestration for parallel, sequential, and fault-tolerant
execution.

> **Rust implementation (formerly Wave 2 de-Pythonification, Epic #511).** The
> orchestration modules are implemented in the Rust crate
> `crates/amplihack-orchestration`. The orchestration crate is a library used
> by other Rust components; no shell-callable CLI is exposed.

**Crate**: [`amplihack-orchestration`](../../crates/amplihack-orchestration/)

**Module map**:

| Module | Crate path |
|---|---|
| `claude_process` | `amplihack_orchestration::claude_process` |
| `execution` | `amplihack_orchestration::execution` |
| `session` | `amplihack_orchestration::session` |
| `patterns::n_version` | `amplihack_orchestration::patterns::n_version` |
| `patterns::debate` | `amplihack_orchestration::patterns::debate` |
| `patterns::cascade` | `amplihack_orchestration::patterns::cascade` |
| `patterns::expert_panel` | `amplihack_orchestration::patterns::expert_panel` |

The public API exposes `ProcessRunner`, `ClaudeProcess`,
`run_parallel`, `run_sequential`, `run_with_fallback`, `run_batched`,
`OrchestratorSession`, `run_n_version`, `run_debate`, `run_cascade`,
`create_custom_cascade`, `run_expert_panel`. See the crate's `tests/` for
behavioral examples covering all four patterns.

## Memory System

Persistent memory storage for agents with session management.

### Agent Memory Interface

**File**: `crates/amplihack-memory/src/interface.rs`

**Purpose**: Simple agent memory API following bricks & studs philosophy

**Usage**:

```rust
use amplihack_memory::AgentMemory;

// Create memory for agent
let mut memory = AgentMemory::new("my-agent", Some("session_123"))?;

// Store data
memory.store("user-pref", "dark-mode")?;
memory.store_json("config", &serde_json::json!({"theme": "dark"}))?;

// Retrieve data
let value = memory.retrieve("user-pref")?;  // "dark-mode"

// List keys
let keys = memory.list_keys(None)?;           // ["user-pref", "config"]
let keys = memory.list_keys(Some("user-*"))?; // ["user-pref"]

// Delete data
memory.delete("old-key")?;

// Clear session
memory.clear_session()?;

// Get statistics
let stats = memory.get_stats()?;
println!("Keys: {}", stats.key_count);

// RAII scoped usage
{
    let memory = AgentMemory::new("my-agent", None)?;
    memory.store("temp", "value")?;
    // Automatically closed on drop
}
```

**Features**:

- SQLite-based backend (`~/.amplihack/.claude/runtime/memory.db`)
- Session-scoped memory
- Optional activation (enabled by default)
- Performance guarantees (< 100ms operations)
- Safe concurrent access

### Memory Backend

**File**: `crates/amplihack-memory/src/core.rs`

**Purpose**: Low-level SQLite-based memory storage

**Schema**:

```sql
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    agent_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_accessed TEXT NOT NULL
);

CREATE TABLE memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    memory_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);
```

## Session Management

Unified session lifecycle management for Claude Code workflows.

### Session Toolkit

**Crate**: `amplihack-session` (`crates/amplihack-session/`)

> **Note (rysweet/amplihack-rs#532):** the bundled `amplihack/session/`
> tree was removed. Session management is provided by the native Rust
> `amplihack-session` crate.

**Purpose**: Single interface for all session management capabilities

**Usage**:

```rust
use amplihack_session::{quick_session, SessionConfig, SessionToolkit};
use std::path::PathBuf;

// Create toolkit
let mut toolkit = SessionToolkit::new(
    PathBuf::from(".claude/runtime"),
    true,        // auto_save
    "INFO",
)?;

// RAII helper for scoped session management.
quick_session("analysis_task", |toolkit, sid| {
    let session = toolkit.manager_mut().get_session(sid).unwrap();
    let _ = session.execute_command("analyze code", None, serde_json::json!({}))?;
    Ok::<_, amplihack_session::SessionError>(())
})?;

// List sessions
let active = toolkit.list_sessions(true);

// Resume session
toolkit.resume_session("existing-id")?;

// Statistics & cleanup
let stats = toolkit.get_toolkit_stats();
let cleaned = toolkit.cleanup_old_data(30, 7, 24)?;
```

**Modules** (under `crates/amplihack-session/src/`):

- `session.rs` - `ClaudeSession` + `CommandExecutor` trait (uses explicit
  `check_health()` for health monitoring).
- `manager.rs` - `SessionManager`: persist / resume / archive / cleanup
  (auto-save thread replaced with explicit `save_all_active()`).
- `logger.rs` - `ToolkitLogger`: structured JSON-line writer with
  size+date rotation and `OperationContext` RAII timing.
- `file_utils.rs` + `batch.rs` - `safe_read_json` / `safe_write_json`,
  checksums, and `BatchFileOperations`.

## Reflection System

AI-powered session analysis and improvement suggestions.

### Reflection Analysis

**File**: `crates/amplihack-reflection/src/reflection.rs`

**Purpose**: Analyze sessions and create GitHub issues for improvements

**Usage**:

```rust
use amplihack_reflection::process_reflection_analysis;

// Analyze session messages
let messages = vec![
    Message { content: "Error: module not found".into() },
    Message { content: "Fixed by updating imports".into() },
];

// Process analysis
if let Some(issue_number) = process_reflection_analysis(&messages)? {
    println!("Created issue: #{issue_number}");
}
```

**Features**:

- Contextual error pattern detection
- Workflow issue identification
- Automation opportunity detection
- Semantic duplicate detection
- GitHub issue creation with labels
- Content sanitization for security

**Environment Variables**:

- `REFLECTION_ENABLED` - Enable/disable reflection (default: true)
- `AMPLIHACK_DEBUG` - Show full stack traces in errors

### Semantic Duplicate Detector

**File**: `crates/amplihack-reflection/src/semantic_duplicate_detector.rs`

**Purpose**: Detect duplicate GitHub issues before creation

**Features**:

- Vector-based similarity analysis
- Issue caching for performance
- Configurable similarity threshold
- Type-based filtering

### Contextual Error Analyzer

**File**: `crates/amplihack-reflection/src/contextual_error_analyzer.rs`

**Purpose**: Analyze error patterns with context awareness

**Features**:

- Pattern library for common errors
- Context extraction
- Priority assignment
- Actionable suggestions

## Utilities

### CI Status Checker

**File**: `ci_status.rs`

**Usage**:

```rust
use amplihack_tools::ci_status::check_ci_status;

// Check current branch
let status = check_ci_status(None)?;

// Check specific PR
let status = check_ci_status(Some("123"))?;

println!("Status: {}", status.status);
println!("URL: {}", status.url);
```

### GitHub Issue Creation

**File**: `github_issue.rs`

**Usage**:

```rust
use amplihack_tools::github_issue::create_issue;

let result = create_issue(
    "Bug report",
    "Details here",
    &["bug", "priority-high"],
)?;

println!("Issue URL: {}", result.url);
```

### CI/Pre-commit Workflows

**Files**:

- `ci_workflow.rs` - CI diagnostic and fix workflow
- `precommit_workflow.rs` - Pre-commit diagnostic workflow

**Purpose**: Automated diagnostics for CI and pre-commit failures

## Quick Start

### 1. Basic Hook Usage

> **Note (rysweet/amplihack-rs#285):** Claude Code hooks are provided
> by the Rust `amplihack-hooks` crate; see that crate for the current API.

### 2. Session Management

```rust
use amplihack_session::{quick_session, SessionToolkit};

let mut toolkit = SessionToolkit::new(".claude/runtime", true, "INFO")?;

quick_session("my_task", |toolkit, sid| {
    toolkit.logger().info("Task started", None)?;
    // ...your work here...
    Ok::<_, amplihack_session::SessionError>(())
})?;
```

### 3. Memory Storage

```rust
use amplihack_memory::AgentMemory;

{
    let memory = AgentMemory::new("my-agent", None)?;
    memory.store_json("config", &serde_json::json!({"theme": "dark"}))?;
    let config = memory.retrieve("config")?;
}
```

### 4. Orchestration

```rust
use amplihack_orchestration::{ClaudeProcess, run_parallel};
use std::path::PathBuf;

let cwd = PathBuf::from(".");
let log_dir = PathBuf::from(".claude/runtime/logs");

let processes = vec![
    ClaudeProcess::new("analyze security", "p1", &cwd, &log_dir),
    ClaudeProcess::new("analyze performance", "p2", &cwd, &log_dir),
    ClaudeProcess::new("analyze code quality", "p3", &cwd, &log_dir),
];

let results = run_parallel(&processes, 3)?;
```

## Integration Examples

### Complete Workflow Example

```rust
use amplihack_session::{quick_session, SessionToolkit};
use amplihack_memory::AgentMemory;
use amplihack_orchestration::{ClaudeProcess, run_parallel};
use std::path::PathBuf;

// For a full example see `crates/amplihack-session/examples/advanced_scenarios.rs`.

let mut toolkit = SessionToolkit::new(
    PathBuf::from(".claude/runtime"),
    true,
    "INFO",
)?;

quick_session("comprehensive_analysis", |toolkit, sid| {
    let logger = toolkit.logger();
    logger.info("Starting comprehensive analysis", None)?;

    // Use memory to store configuration
    let memory = AgentMemory::new("analysis-agent", Some(sid))?;
    memory.store_json("analysis_config", &serde_json::json!({
        "depth": "deep",
        "targets": ["security", "performance", "quality"]
    }))?;

    // Create parallel analysis processes
    let targets = ["security", "performance", "quality"];
    let cwd = PathBuf::from(".");
    let log_dir = PathBuf::from(".claude/runtime/logs");

    let processes: Vec<_> = targets.iter().map(|target| {
        ClaudeProcess::new(
            &format!("Analyze {target}"),
            &format!("{target}_analysis"),
            &cwd,
            &log_dir,
        )
    }).collect();

    // Execute in parallel
    logger.info(&format!("Running {} analyses in parallel", processes.len()), None)?;
    let results = run_parallel(&processes, 3)?;

    // Process results
    let successful: Vec<_> = results.iter().filter(|r| r.exit_code == 0).collect();
    logger.info(
        &format!("Completed {}/{} analyses", successful.len(), results.len()),
        None,
    )?;

    // Store results in memory
    for result in &successful {
        memory.store_json(
            &format!("result_{}", result.process_id),
            &serde_json::json!({
                "output": result.output,
                "duration": result.duration
            }),
        )?;
    }

    // Session stats
    let stats = toolkit.get_toolkit_stats();
    logger.info(&format!("Session stats: {stats:?}"), None)?;

    Ok::<_, amplihack_session::SessionError>(())
})?;
```

### Custom Hook Example

> **Note (rysweet/amplihack-rs#285):** the `amplihack.hooks.hook_processor`
> base class was removed alongside the bundled `amplihack/hooks/`
> tree. Custom hooks are now implemented in the Rust `amplihack-hooks`
> crate; refer to that crate for the current extension API.

## Performance Considerations

### Hook Performance

- **session_start**: < 500ms (includes file I/O)
- **user_prompt_submit**: < 50ms (cached preferences)
- **post_tool_use**: < 10ms (async metrics)
- **pre_compact**: < 1s (depends on conversation size)
- **stop**: < 100ms (simple flag check)

### Memory Operations

- **store/retrieve**: < 100ms
- **list_keys**: < 200ms
- **clear_session**: < 500ms

### Orchestration

- **Parallel**: Near-linear scaling up to system limits
- **Sequential**: Additive (sum of process times)
- **Fallback**: Best case = first process time

## Security

### Path Validation

All file operations use `validate_path_containment()` to prevent path traversal attacks.

### Content Sanitization

Reflection system uses `security.rs` to sanitize all content before GitHub issue creation.

### XPIA Defense

`xpia_defense.rs` provides Cross-Process Injection Attack protection.

### Permissions

- Session directories created with `0o700` (owner-only)
- Log files rotated at 10MB to prevent disk exhaustion
- Metrics stored in append-only JSONL format

## Troubleshooting

### Hook Not Running

> **Note (rysweet/amplihack-rs#285):** hooks are implemented in the Rust
> `amplihack-hooks` crate, not as files under `.claude/tools/`.
> Refer to that crate's diagnostics and the runtime logs in
> `~/.amplihack/.claude/runtime/logs/` when troubleshooting.

### Memory Not Persisting

1. Check `~/.amplihack/.claude/runtime/memory.db` exists
2. Verify session_id is consistent
3. Check file permissions
4. Confirm `enabled: true` when creating AgentMemory

### Orchestration Timeouts

1. Increase timeout in ClaudeProcess constructor
2. Check system resources (CPU, memory)
3. Review logs in process log directories
4. Consider batched execution for large workloads

## Contributing

When adding new tools to this directory:

1. **Follow the structure**: Place tools in appropriate subdirectories
2. **Use HookProcessor**: Extend the base class for new hooks
3. **Document thoroughly**: Update this README with new capabilities
4. **Add examples**: Provide usage examples in docstrings
5. **Write tests**: Add tests to `tests/` subdirectories
6. **Security first**: Validate paths, sanitize content, handle errors gracefully

## References

- [Claude Code Hooks Documentation](https://docs.claude.com/en/docs/claude-code/hooks)
- [Project Philosophy](~/.amplihack/.claude/context/PHILOSOPHY.md)
- [Development Patterns](~/.amplihack/.claude/context/PATTERNS.md)
- [`default-workflow` skill/recipe](../skills/default-workflow/SKILL.md)

---

**Last Updated**: 2025-01-05
**Maintainer**: Amplihack Framework Team
**Questions**: Check `~/.amplihack/.claude/context/DISCOVERIES.md` for known issues and solutions
