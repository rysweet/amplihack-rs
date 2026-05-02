# Session Management Toolkit (Rust)

> **Deyhonification wave 3b (rysweet/amplihack-rs#532):** the Python session
> toolkit that previously lived in this directory has been removed. Session
> management is now provided by the native Rust crate
> [`amplihack-session`](../../../../crates/amplihack-session) under
> `crates/amplihack-session/`.

## Overview

`amplihack-session` is a synchronous, dependency-light Rust port of the
former `amplihack/session/` Python package. It provides:

- **`ClaudeSession`** — session lifecycle wrapper with command history,
  checkpoints, and `check_health()` (replaces the Python heartbeat thread).
- **`SessionManager`** — multi-session registry with persist / resume /
  archive / `cleanup_old_sessions()` and explicit `save_all_active()`
  (replaces the Python auto-save thread).
- **`ToolkitLogger`** — structured JSON-line logging with size-based
  rotation, child loggers, and `OperationContext` RAII timing.
- **`SessionToolkit`** — facade that wires the above together; pair it
  with the `quick_session()` helper for `with`-style ergonomics.
- **`safe_read_json` / `safe_write_json`** — atomic file operations with
  default-on-missing semantics and a 64 MiB read cap.
- **`BatchFileOperations`** — batched, base-directory-rooted writes that
  reject absolute paths and `..` traversal.

## Quick Start

```rust
use amplihack_session::{quick_session, SessionConfig, SessionToolkit};

fn main() -> Result<(), amplihack_session::SessionError> {
    let mut toolkit = SessionToolkit::new(".claude/runtime", true, "INFO")?;

    let id = toolkit.create_session("analysis", None, None)?;
    if let Some(s) = toolkit.manager_mut().get_session(&id) {
        s.start();
        let _ = s.execute_command("hello", None, serde_json::json!({}))?;
        s.stop();
    }
    toolkit.save_current()?;

    // RAII helper (Python `with toolkit.session(...)` analog):
    quick_session("scoped", |toolkit, sid| {
        let session = toolkit.manager_mut().get_session(sid).unwrap();
        let _ = session.execute_command("scoped-work", None, serde_json::json!({}))?;
        Ok::<_, amplihack_session::SessionError>(())
    })?;
    Ok(())
}
```

See `crates/amplihack-session/examples/` for `basic_usage.rs` and
`advanced_scenarios.rs` (which demonstrates a real shell-backed
`CommandExecutor` impl, child loggers, and export/import round trip).

## Migration Notes

| Python API                                     | Rust replacement                                          |
| ---------------------------------------------- | --------------------------------------------------------- |
| `with toolkit.session("x") as s:`              | `quick_session("x", |toolkit, sid| { ... })`              |
| Heartbeat thread                                | Explicit `session.check_health(now)` (call as needed)     |
| Auto-save thread                                | Explicit `manager.save_all_active()` (and `Drop` does it) |
| `_simulate_command_execution()`                 | `trait CommandExecutor` + `NoopExecutor` default          |
| `safe_read_json(path, default=...)`             | `safe_read_json(path)` returns `default` on missing/invalid; large files (>64 MiB) → `SessionError::TooLarge` |
| `BatchFileOperations.write_json(...)`           | Same; rejects absolute paths and `..` components          |

## Status

Sync-only in v1. An async wrapper may be considered later if a caller
requires it; current tests do not exercise async behavior.
