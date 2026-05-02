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
│   │   ├── claude_transcript_builder.py    # Session transcripts
│   │   ├── codex_transcripts_builder.py    # Codex-optimized exports
│   │   └── export_on_compact_integration.py # Compaction integration
│   ├── orchestration/          # Multi-process orchestration
│   │   ├── claude_process.py   # Single process execution
│   │   ├── execution.py        # Parallel/sequential/fallback execution
│   │   ├── session.py          # Session coordination
│   │   └── patterns/           # Fault tolerance patterns
│   │       ├── n_version.py    # N-version programming
│   │       ├── debate.py       # Multi-agent debate
│   │       ├── cascade.py      # Fallback cascade
│   │       └── expert_panel.py # Expert consensus
│   ├── memory/                 # Agent memory system
│   │   ├── interface.py        # Clean API for memory operations
│   │   ├── core.py             # SQLite-based backend
│   │   ├── context_preservation.py # Context management
│   │   └── examples/           # Usage examples
│   ├── reflection/             # AI-powered reflection system
│   │   ├── reflection.py       # Session analysis and improvement
│   │   ├── semantic_duplicate_detector.py # Issue deduplication
│   │   ├── contextual_error_analyzer.py   # Error pattern analysis
│   │   ├── security.py         # Content sanitization
│   │   └── display.py          # User-facing output
│   ├── session/                # Session lifecycle management
│   │   ├── session_toolkit.py  # Unified session interface
│   │   ├── claude_session.py   # Core session implementation
│   │   ├── session_manager.py  # Multi-session coordination
│   │   ├── toolkit_logger.py   # Session logging
│   │   └── file_utils.py       # Safe file operations
│   ├── context_preservation.py # Context extraction and export
│   ├── xpia_defense.py         # Cross-Process Injection Attack defense
│   └── paths.py                # Path utilities
├── ci_status.py                # CI/CD status checking
├── ci_workflow.py              # CI diagnostic workflow
├── precommit_workflow.py       # Pre-commit diagnostic workflow
├── github_issue.py             # GitHub issue creation
├── improvement_validator.py    # Improvement validation
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

#### `session_start.py`

**Purpose**: Initialize session with context and preferences

**What it does**:

- Injects project context (PHILOSOPHY.md, DISCOVERIES.md)
- Loads and enforces USER_PREFERENCES.md (MANDATORY)
- Captures original request for context preservation
- Stages UVX framework if deployed via uvx
- Provides workflow information

**Returns**: `{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "..."}}`

**Example**:

```python
# Automatically triggered by Claude Code on session start
# Injects context visible to Claude in the conversation
```

#### `user_prompt_submit.py`

**Purpose**: Inject user preferences on every user message

**What it does**:

- Reads USER_PREFERENCES.md on each user prompt
- Extracts key preferences (communication style, verbosity, etc.)
- Caches preferences for performance (invalidates on file change)
- Injects concise preference context to enforce behavior

**Returns**: `{"additionalContext": "🎯 ACTIVE USER PREFERENCES (MANDATORY): ..."}`

**Example**:

```python
# Automatically triggered on every user message
# Ensures preferences persist across conversation turns
```

#### `post_tool_use.py`

**Purpose**: Track tool usage and collect metrics

**What it does**:

- Logs every tool invocation
- Saves structured metrics (tool name, duration)
- Categorizes tools (bash, file operations, search)
- Detects and logs tool errors

**Returns**: `{}` or `{"metadata": {"warning": "..."}}`

**Example**:

```python
# Automatically triggered after each tool use
# Metrics saved to .claude/runtime/metrics/
```

#### `pre_compact.py`

**Purpose**: Export conversation before context compaction

**What it does**:

- Receives full conversation history from Claude Code
- Exports to CONVERSATION_TRANSCRIPT.md
- Preserves original request if available
- Creates timestamped backup copies
- Saves compaction event metadata

**Returns**: `{"status": "success", "message": "...", "transcript_path": "..."}`

**Example**:

```python
# Automatically triggered before Claude Code compacts context
# Ensures no conversation history is lost
```

#### `stop.py`

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

```python
# Automatically triggered when Claude tries to stop
# Lock flag enables continuous multi-turn work
```

### Base Hook Processor

> **Note (rysweet/amplihack-rs#285):** the bundled `amplihack/hooks/` Python
> tree (including `hook_processor.py`) was removed. Hook orchestration in
> amplihack-rs is implemented in the Rust `amplihack-hooks` crate; consult
> that crate for the current API.

## Builders

Builders create structured documentation and exports from session data.

### Claude Transcript Builder

**File**: `amplihack/builders/claude_transcript_builder.py`

**Purpose**: Build comprehensive session transcripts for documentation and knowledge extraction

**Usage**:

```python
from amplihack.builders.claude_transcript_builder import ClaudeTranscriptBuilder

# Create builder
builder = ClaudeTranscriptBuilder(session_id="20250105_143022")

# Build transcript
transcript_path = builder.build_session_transcript(
    messages=[
        {"role": "user", "content": "Hello", "timestamp": "2025-01-05T14:30:22"},
        {"role": "assistant", "content": "Hi!", "timestamp": "2025-01-05T14:30:23"},
    ],
    metadata={"project": "amplihack"}
)

# Build session summary
summary = builder.build_session_summary(messages, metadata)
print(f"Total words: {summary['total_words']}")

# Export for codex
codex_path = builder.export_for_codex(messages, metadata)
```

**Outputs**:

- `CONVERSATION_TRANSCRIPT.md` - Human-readable markdown transcript
- `conversation_transcript.json` - Machine-readable JSON format
- `session_summary.json` - Statistical summary
- `codex_export.json` - Knowledge extraction optimized format

### Codex Transcripts Builder

**File**: `amplihack/builders/codex_transcripts_builder.py`

**Purpose**: Create codex-optimized exports for knowledge systems

**Features**:

- Pattern detection (tool usage, error-fix cycles)
- Decision extraction
- Knowledge artifact identification
- Conversation flow analysis

## Orchestration

Multi-process orchestration for parallel, sequential, and fault-tolerant
execution.

> **Native Rust port (Wave 2 deyhonification, Epic #511).** The Python modules
> previously located at `amplihack/orchestration/` have been ported to the
> Rust crate `crates/amplihack-orchestration` and the original `.py` files
> deleted. Use the Rust API directly from the workspace; no shell-callable
> CLI is exposed (the orchestration crate is a library used by other Rust
> components).

**Crate**: [`amplihack-orchestration`](../../crates/amplihack-orchestration/)

**Migration map** (Python → Rust):

| Python path | Rust path |
|---|---|
| `orchestration.claude_process` | `amplihack_orchestration::claude_process` |
| `orchestration.execution` | `amplihack_orchestration::execution` |
| `orchestration.session` | `amplihack_orchestration::session` |
| `orchestration.patterns.n_version` | `amplihack_orchestration::patterns::n_version` |
| `orchestration.patterns.debate` | `amplihack_orchestration::patterns::debate` |
| `orchestration.patterns.cascade` | `amplihack_orchestration::patterns::cascade` |
| `orchestration.patterns.expert_panel` | `amplihack_orchestration::patterns::expert_panel` |

The Rust API mirrors the Python public surface (`ProcessRunner`, `ClaudeProcess`,
`run_parallel`, `run_sequential`, `run_with_fallback`, `run_batched`,
`OrchestratorSession`, `run_n_version`, `run_debate`, `run_cascade`,
`create_custom_cascade`, `run_expert_panel`). See the crate's `tests/` for
behavioral examples covering all four patterns.

## Memory System

Persistent memory storage for agents with session management.

### Agent Memory Interface

**File**: `amplihack/memory/interface.py`

**Purpose**: Simple agent memory API following bricks & studs philosophy

**Usage**:

```python
from amplihack.memory.interface import AgentMemory

# Create memory for agent
memory = AgentMemory("my-agent", session_id="session_123")

# Store data
memory.store("user-pref", "dark-mode")
memory.store("config", {"theme": "dark"}, memory_type="json")

# Retrieve data
value = memory.retrieve("user-pref")  # "dark-mode"

# List keys
keys = memory.list_keys()               # ['user-pref', 'config']
keys = memory.list_keys("user-*")       # ['user-pref']

# Delete data
memory.delete("old-key")

# Clear session
memory.clear_session()

# Get statistics
stats = memory.get_stats()
print(f"Keys: {stats['key_count']}")

# Context manager usage
with AgentMemory("my-agent") as memory:
    memory.store("temp", "value")
    # Automatically closed on exit
```

**Features**:

- SQLite-based backend (`~/.amplihack/.claude/runtime/memory.db`)
- Session-scoped memory
- Optional activation (enabled by default)
- Performance guarantees (< 100ms operations)
- Safe concurrent access

### Memory Backend

**File**: `amplihack/memory/core.py`

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
> Python tree was removed in deyhonification wave 3b. Session management
> is now provided by the native Rust `amplihack-session` crate.

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

// RAII helper (mirrors the Python `with toolkit.session(...)` block).
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

- `session.rs` - `ClaudeSession` + `CommandExecutor` trait (replaces the
  Python heartbeat thread with explicit `check_health()`).
- `manager.rs` - `SessionManager`: persist / resume / archive / cleanup
  (auto-save thread replaced with explicit `save_all_active()`).
- `logger.rs` - `ToolkitLogger`: structured JSON-line writer with
  size+date rotation and `OperationContext` RAII timing.
- `file_utils.rs` + `batch.rs` - `safe_read_json` / `safe_write_json`,
  checksums, and `BatchFileOperations`.

## Reflection System

AI-powered session analysis and improvement suggestions.

### Reflection Analysis

**File**: `amplihack/reflection/reflection.py`

**Purpose**: Analyze sessions and create GitHub issues for improvements

**Usage**:

```python
from amplihack.reflection.reflection import process_reflection_analysis

# Analyze session messages
messages = [
    {"content": "Error: module not found"},
    {"content": "Fixed by updating imports"},
]

# Process analysis
issue_number = process_reflection_analysis(messages)

if issue_number:
    print(f"Created issue: #{issue_number}")
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

**File**: `amplihack/reflection/semantic_duplicate_detector.py`

**Purpose**: Detect duplicate GitHub issues before creation

**Features**:

- Vector-based similarity analysis
- Issue caching for performance
- Configurable similarity threshold
- Type-based filtering

### Contextual Error Analyzer

**File**: `amplihack/reflection/contextual_error_analyzer.py`

**Purpose**: Analyze error patterns with context awareness

**Features**:

- Pattern library for common errors
- Context extraction
- Priority assignment
- Actionable suggestions

## Utilities

### CI Status Checker

**File**: `ci_status.py`

**Usage**:

```python
from .claude.tools.ci_status import check_ci_status

# Check current branch
status = check_ci_status()

# Check specific PR
status = check_ci_status(ref="123")

print(f"Status: {status['status']}")
print(f"URL: {status['url']}")
```

### GitHub Issue Creation

**File**: `github_issue.py`

**Usage**:

```python
from .claude.tools.github_issue import create_issue

result = create_issue(
    title="Bug report",
    body="Details here",
    labels=["bug", "priority-high"]
)

print(f"Issue URL: {result['url']}")
```

### CI/Pre-commit Workflows

**Files**:

- `ci_workflow.py` - CI diagnostic and fix workflow
- `precommit_workflow.py` - Pre-commit diagnostic workflow

**Purpose**: Automated diagnostics for CI and pre-commit failures

## Quick Start

### 1. Basic Hook Usage

> **Note (rysweet/amplihack-rs#285):** Claude Code hooks are now provided
> by the Rust `amplihack-hooks` crate; the bundled `amplihack/hooks/`
> Python directory was removed.

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

```python
from amplihack.memory.interface import AgentMemory

with AgentMemory("my-agent") as memory:
    memory.store("config", {"theme": "dark"})
    config = memory.retrieve("config")
```

### 4. Orchestration

```python
from amplihack.orchestration.claude_process import ClaudeProcess
from amplihack.orchestration.execution import run_parallel

processes = [
    ClaudeProcess("analyze security", "p1", cwd, log_dir),
    ClaudeProcess("analyze performance", "p2", cwd, log_dir),
    ClaudeProcess("analyze code quality", "p3", cwd, log_dir),
]

results = run_parallel(processes, max_workers=3)
```

## Integration Examples

### Complete Workflow Example

```python
# NOTE: session management has been ported to the Rust `amplihack-session`
# crate (rysweet/amplihack-rs#532). The example below shows how the
# remaining Python-side helpers compose with the Rust toolkit via shell
# invocation. For a pure-Rust example see
# `crates/amplihack-session/examples/advanced_scenarios.rs`.
from pathlib import Path
from amplihack.memory.interface import AgentMemory
from amplihack.orchestration.claude_process import ClaudeProcess
from amplihack.orchestration.execution import run_parallel

# Session lifecycle is now driven by the Rust `amplihack-session` crate;
# the Python placeholder below is illustrative only.
toolkit = None  # Use the Rust SessionToolkit, see crates/amplihack-session

with toolkit.session("comprehensive_analysis") as session:
    logger = toolkit.get_logger("main")
    logger.info("Starting comprehensive analysis")

    # Use memory to store configuration
    memory = AgentMemory("analysis-agent", session_id=session.session_id)
    memory.store("analysis_config", {
        "depth": "deep",
        "targets": ["security", "performance", "quality"]
    })

    # Create parallel analysis processes
    config = memory.retrieve("analysis_config")
    processes = [
        ClaudeProcess(
            f"Analyze {target}",
            f"{target}_analysis",
            Path.cwd(),
            Path(".claude/runtime/logs")
        )
        for target in config["targets"]
    ]

    # Execute in parallel
    logger.info(f"Running {len(processes)} analyses in parallel")
    results = run_parallel(processes, max_workers=3)

    # Process results
    successful = [r for r in results if r.exit_code == 0]
    logger.info(f"Completed {len(successful)}/{len(results)} analyses")

    # Store results in memory
    for result in successful:
        memory.store(
            f"result_{result.process_id}",
            {"output": result.output, "duration": result.duration}
        )

    # Session stats
    stats = toolkit.get_session_stats()
    logger.info(f"Session stats: {stats}")
```

### Custom Hook Example

> **Note (rysweet/amplihack-rs#285):** the `amplihack.hooks.hook_processor`
> Python base class was removed alongside the bundled `amplihack/hooks/`
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

Reflection system uses `security.py` to sanitize all content before GitHub issue creation.

### XPIA Defense

`xpia_defense.py` provides Cross-Process Injection Attack protection.

### Permissions

- Session directories created with `0o700` (owner-only)
- Log files rotated at 10MB to prevent disk exhaustion
- Metrics stored in append-only JSONL format

## Troubleshooting

### Hook Not Running

> **Note (rysweet/amplihack-rs#285):** hooks are implemented in the Rust
> `amplihack-hooks` crate, not as Python files under `.claude/tools/`.
> Refer to that crate's diagnostics and the runtime logs in
> `~/.amplihack/.claude/runtime/logs/` when troubleshooting.

### Memory Not Persisting

1. Check `~/.amplihack/.claude/runtime/memory.db` exists
2. Verify session_id is consistent
3. Check file permissions
4. Confirm `enabled=True` when creating AgentMemory

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
- [Workflow Definition](~/.amplihack/.claude/workflow/DEFAULT_WORKFLOW.md)

---

**Last Updated**: 2025-01-05
**Maintainer**: Amplihack Framework Team
**Questions**: Check `~/.amplihack/.claude/context/DISCOVERIES.md` for known issues and solutions
