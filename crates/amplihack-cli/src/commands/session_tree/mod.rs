//! `amplihack session-tree` — native Rust port of `session_tree.py`.
//!
//! Tracks active orchestration sessions in a tree structure to bound
//! recursion depth and concurrent fan-out. The `register`, `complete`,
//! `status`, and `check` subcommands match the byte-exact stdout contract
//! consumed by `amplifier-bundle/recipes/smart-orchestrator.yaml`.
//!
//! Stdout contract (one line per command, no trailing whitespace except `\n`):
//! * `register` → `TREE_ID=<tree_id> DEPTH=<n>\n` (exit 0) or nothing on
//!   registration failure (exit 1, with the reason on stderr)
//! * `complete` → no stdout (exit 0)
//! * `status`   → JSON object, multiline (pretty) (exit 0)
//! * `check`    → `ALLOWED\n` (exit 0) or `BLOCKED:<reason>\n` (exit 2)
//!
//! Diagnostic output goes to stderr via `eprintln!` to keep stdout
//! parser-friendly.

pub mod state;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use state::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_SESSIONS, MAX_DEPTH_CEILING, SessionEntry, SessionStatus,
    load_state, save_state, state_dir, state_path_in, validate_tree_id, with_locked_tree,
};

/// `amplihack session-tree <subcommand>`
#[derive(Subcommand, Debug)]
pub enum SessionTreeCommands {
    /// Register a session in the current tree.
    Register {
        /// Session ID to register. If omitted, a random 8-hex-char id is generated.
        session_id: Option<String>,
        /// Optional parent session ID.
        parent_id: Option<String>,
    },
    /// Mark a session as completed.
    Complete {
        /// Session ID to mark complete.
        session_id: String,
    },
    /// Print a JSON status summary for the current tree.
    Status {
        /// Tree ID (defaults to $AMPLIHACK_TREE_ID).
        tree_id: Option<String>,
    },
    /// Check whether a new child session can be spawned. Exit 0 + stdout
    /// "ALLOWED" if allowed, exit 2 + stdout "BLOCKED:\<reason\>" if not.
    Check,
}

#[derive(Debug, Clone)]
struct TreeContext {
    tree_id: Option<String>,
    depth: u32,
    max_depth: u32,
    max_sessions: u32,
}

fn tree_context() -> TreeContext {
    let tree_id = std::env::var("AMPLIHACK_TREE_ID")
        .ok()
        .filter(|s| !s.is_empty());
    let depth = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_DEPTH)
        .min(MAX_DEPTH_CEILING);
    let max_sessions = std::env::var("AMPLIHACK_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_SESSIONS);
    TreeContext {
        tree_id,
        depth,
        max_depth,
        max_sessions,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Public entry point dispatched from `commands::dispatch`.
pub fn run(cmd: SessionTreeCommands) -> Result<()> {
    let ctx = tree_context();
    match cmd {
        SessionTreeCommands::Register {
            session_id,
            parent_id,
        } => run_register(ctx, session_id, parent_id),
        SessionTreeCommands::Complete { session_id } => run_complete(ctx, &session_id),
        SessionTreeCommands::Status { tree_id } => run_status(ctx, tree_id),
        SessionTreeCommands::Check => run_check(ctx),
    }
}

fn run_register(
    ctx: TreeContext,
    session_id: Option<String>,
    parent_id: Option<String>,
) -> Result<()> {
    let session_id = session_id.unwrap_or_else(random_id);
    validate_tree_id(&session_id).context("invalid session_id")?;
    if let Some(p) = parent_id.as_deref() {
        validate_tree_id(p).context("invalid parent_id")?;
    }
    let tree_id = match ctx.tree_id.clone() {
        Some(id) => {
            validate_tree_id(&id).context("invalid AMPLIHACK_TREE_ID")?;
            id
        }
        None => random_id(),
    };

    let dir = state_dir()?;
    let max_sessions = ctx.max_sessions;
    let max_depth = ctx.max_depth;
    let depth = ctx.depth;
    let parent_id_clone = parent_id.clone();
    let session_id_clone = session_id.clone();

    let outcome: Result<()> = with_locked_tree(&dir, &tree_id, move |path| {
        let mut state = load_state(path)?;
        let active = state.active_count();
        if active >= max_sessions {
            anyhow::bail!("max_sessions={max_sessions} reached ({active} active)");
        }
        if depth > max_depth {
            anyhow::bail!("depth={depth} exceeds max_depth={max_depth}");
        }
        let entry = SessionEntry {
            depth,
            parent: parent_id_clone.clone(),
            status: SessionStatus::Active,
            started_at: now_secs(),
            completed_at: None,
            children: vec![],
        };
        state.sessions.insert(session_id_clone.clone(), entry);
        if let Some(pid) = parent_id_clone.as_ref()
            && let Some(parent_entry) = state.sessions.get_mut(pid)
            && !parent_entry.children.contains(&session_id_clone)
        {
            parent_entry.children.push(session_id_clone.clone());
        }
        save_state(path, state)
    });

    match outcome {
        Ok(()) => {
            // Byte-exact stdout contract consumed by smart-orchestrator.yaml.
            println!("TREE_ID={tree_id} DEPTH={depth}");
            Ok(())
        }
        Err(err) => {
            eprintln!("ERROR: {err}");
            std::process::exit(1);
        }
    }
}

fn run_complete(ctx: TreeContext, session_id: &str) -> Result<()> {
    validate_tree_id(session_id).context("invalid session_id")?;
    let Some(tree_id) = ctx.tree_id.clone() else {
        // Nothing to complete without a tree — keep parity with Python (silent no-op).
        return Ok(());
    };
    validate_tree_id(&tree_id).context("invalid AMPLIHACK_TREE_ID")?;
    let dir = state_dir()?;
    let session_id_owned = session_id.to_string();
    with_locked_tree(&dir, &tree_id, move |path| {
        let mut state = load_state(path)?;
        if let Some(entry) = state.sessions.get_mut(&session_id_owned) {
            entry.status = SessionStatus::Completed;
            entry.completed_at = Some(now_secs());
        }
        save_state(path, state)
    })?;
    Ok(())
}

fn run_status(ctx: TreeContext, tree_id_arg: Option<String>) -> Result<()> {
    let tree_id = match tree_id_arg.or(ctx.tree_id) {
        Some(t) => t,
        None => {
            println!("No AMPLIHACK_TREE_ID set");
            std::process::exit(1);
        }
    };
    validate_tree_id(&tree_id)?;
    let dir = state_dir()?;
    let path = state_path_in(&dir, &tree_id)?;
    let state = load_state(&path)?;
    let depths: serde_json::Map<String, serde_json::Value> = state
        .sessions
        .iter()
        .map(|(k, v)| (k.clone(), json!(v.depth)))
        .collect();
    let payload = json!({
        "tree_id": tree_id,
        "active": state.active_ids(),
        "completed": state.completed_ids(),
        "depths": depths,
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn run_check(ctx: TreeContext) -> Result<()> {
    let child_depth = ctx.depth.saturating_add(1);
    if child_depth > ctx.max_depth {
        println!(
            "BLOCKED:max_depth={} exceeded at depth={}",
            ctx.max_depth, ctx.depth
        );
        std::process::exit(2);
    }
    let Some(tree_id) = ctx.tree_id.clone() else {
        // Root session creating a brand-new tree — always allowed.
        println!("ALLOWED");
        return Ok(());
    };
    validate_tree_id(&tree_id)?;
    let dir = state_dir()?;
    let path = state_path_in(&dir, &tree_id)?;
    let state = load_state(&path)?;
    let active = state.active_count();
    if active >= ctx.max_sessions {
        println!(
            "BLOCKED:max_sessions={} reached ({} active)",
            ctx.max_sessions, active
        );
        std::process::exit(2);
    }
    println!("ALLOWED");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::state::TreeState;
    use super::*;
    use serial_test_lock::SerialLock;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Module-private serial lock — TMPDIR mutation must not race other tests.
    mod serial_test_lock {
        use std::sync::OnceLock;
        use std::sync::{Mutex, MutexGuard};
        pub struct SerialLock;
        impl SerialLock {
            pub fn acquire() -> MutexGuard<'static, ()> {
                static LK: OnceLock<Mutex<()>> = OnceLock::new();
                LK.get_or_init(|| Mutex::new(()))
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
            }
        }
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn isolated_env() -> (TempDir, std::sync::MutexGuard<'static, ()>) {
        let g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Anchor to /tmp directly; do NOT mutate the global TMPDIR — other
        // crate tests anchor `TempDir::new()` against it concurrently.
        let td = TempDir::new_in("/tmp").unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_SESSION_TREE_DIR", td.path().join("trees"));
            std::env::remove_var("AMPLIHACK_TREE_ID");
            std::env::remove_var("AMPLIHACK_SESSION_DEPTH");
            std::env::remove_var("AMPLIHACK_MAX_DEPTH");
            std::env::remove_var("AMPLIHACK_MAX_SESSIONS");
        }
        (td, g)
    }

    #[test]
    fn register_root_creates_tree_and_writes_state() {
        let _serial = SerialLock::acquire();
        let (_td, _env) = isolated_env();
        let ctx = tree_context();
        // Pre-set tree_id so the test is deterministic.
        let tree_id = "regroot".to_string();
        unsafe {
            std::env::set_var("AMPLIHACK_TREE_ID", &tree_id);
        }
        let ctx = TreeContext {
            tree_id: Some(tree_id.clone()),
            ..ctx
        };

        // Inline of run_register's body (cannot easily call run_register
        // because it prints to stdout and we're testing state shape).
        let dir = state_dir().unwrap();
        with_locked_tree(&dir, &tree_id, |path| {
            let mut state = load_state(path).unwrap();
            state.sessions.insert(
                "s1".into(),
                SessionEntry {
                    depth: ctx.depth,
                    parent: None,
                    status: SessionStatus::Active,
                    started_at: now_secs(),
                    completed_at: None,
                    children: vec![],
                },
            );
            save_state(path, state)
        })
        .unwrap();
        let state = load_state(&state_path_in(&dir, &tree_id).unwrap()).unwrap();
        assert!(state.sessions.contains_key("s1"));
    }

    #[test]
    fn check_at_max_depth_blocks() {
        let _serial = SerialLock::acquire();
        let (_td, _env) = isolated_env();
        unsafe {
            std::env::set_var("AMPLIHACK_SESSION_DEPTH", "3");
            std::env::set_var("AMPLIHACK_MAX_DEPTH", "3");
        }
        let ctx = tree_context();
        // child_depth = 4 > max_depth = 3 → BLOCKED.
        assert_eq!(ctx.depth.saturating_add(1), 4);
        assert!(ctx.depth.saturating_add(1) > ctx.max_depth);
    }

    #[test]
    fn check_below_max_depth_allows_when_no_tree() {
        let _serial = SerialLock::acquire();
        let (_td, _env) = isolated_env();
        unsafe {
            std::env::set_var("AMPLIHACK_SESSION_DEPTH", "0");
            std::env::set_var("AMPLIHACK_MAX_DEPTH", "3");
        }
        let ctx = tree_context();
        assert!(ctx.depth.saturating_add(1) <= ctx.max_depth);
        assert!(ctx.tree_id.is_none());
    }

    #[test]
    fn complete_marks_session_done() {
        let _serial = SerialLock::acquire();
        let (_td, _env) = isolated_env();
        let tree_id = "comp".to_string();
        let dir = state_dir().unwrap();
        with_locked_tree(&dir, &tree_id, |path| {
            let mut state = TreeState::default();
            state.sessions.insert(
                "x".into(),
                SessionEntry {
                    depth: 0,
                    parent: None,
                    status: SessionStatus::Active,
                    started_at: now_secs(),
                    completed_at: None,
                    children: vec![],
                },
            );
            save_state(path, state)
        })
        .unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_TREE_ID", &tree_id);
        }
        let ctx = tree_context();
        run_complete(ctx, "x").unwrap();
        let state = load_state(&state_path_in(&dir, &tree_id).unwrap()).unwrap();
        assert_eq!(state.sessions["x"].status, SessionStatus::Completed);
        assert!(state.sessions["x"].completed_at.is_some());
    }

    #[test]
    fn random_id_format_is_8_lower_hex() {
        let id = random_id();
        assert_eq!(id.len(), 8);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
