//! Session tree management for amplihack orchestration.
//!
//! Prevents infinite recursion by tracking active sessions in a tree structure.
//! Enforces max depth and max concurrent session limits.
//!
//! Replaces `amplifier-bundle/tools/session_tree.py`.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::SessionTreeCommands;

const DEFAULT_MAX_DEPTH: u32 = 3;
const DEFAULT_MAX_SESSIONS: u32 = 10;
/// Reserved for future `increment-spawn` subcommand.
#[allow(dead_code)]
const DEFAULT_MAX_TREE_SPAWNS: u32 = 50;
const LOCK_TIMEOUT_SECS: f64 = 10.0;
const LOCK_SPIN_MS: u64 = 50;
const COMPLETED_MAX_AGE_HOURS: f64 = 24.0;
const ACTIVE_MAX_AGE_HOURS: f64 = 4.0;

// ─────────────────────────────────────────────────────────────────────────────
// State model
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SessionEntry {
    depth: u32,
    parent: Option<String>,
    status: String,
    started_at: f64,
    #[serde(default)]
    completed_at: f64,
    #[serde(default)]
    children: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TreeState {
    sessions: HashMap<String, SessionEntry>,
    #[serde(default)]
    spawn_count: u32,
}

impl Default for TreeState {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            spawn_count: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Environment context
// ─────────────────────────────────────────────────────────────────────────────

struct TreeContext {
    tree_id: String,
    depth: u32,
    max_depth: u32,
    max_sessions: u32,
}

fn get_tree_context() -> TreeContext {
    let depth = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_DEPTH);
    let max_sessions = std::env::var("AMPLIHACK_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_SESSIONS);
    let tree_id = std::env::var("AMPLIHACK_TREE_ID").unwrap_or_default();

    TreeContext {
        tree_id,
        depth,
        max_depth,
        max_sessions,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Paths & validation
// ─────────────────────────────────────────────────────────────────────────────

fn state_dir() -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join("amplihack-session-trees")
}

fn validate_tree_id(tree_id: &str) -> Result<()> {
    if tree_id.is_empty() {
        bail!("tree_id cannot be empty");
    }
    let re = Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").unwrap();
    if !re.is_match(tree_id) {
        bail!(
            "Invalid tree_id {:?}: must match [a-zA-Z0-9_-]{{1,64}}",
            tree_id
        );
    }
    Ok(())
}

fn ensure_state_dir() -> Result<()> {
    let dir = state_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create state dir: {}", dir.display()))?;
        // Best-effort chmod 0o700
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
    }
    Ok(())
}

fn state_path(tree_id: &str) -> Result<PathBuf> {
    validate_tree_id(tree_id)?;
    ensure_state_dir()?;
    let dir = state_dir();
    let candidate = dir.join(format!("{tree_id}.json"));
    // Path traversal guard: resolve and verify prefix
    let resolved = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    let dir_resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    if !resolved.starts_with(&dir_resolved) && candidate != dir.join(format!("{tree_id}.json")) {
        bail!("Path traversal detected for tree_id {:?}", tree_id);
    }
    Ok(dir.join(format!("{tree_id}.json")))
}

fn lock_path(tree_id: &str) -> Result<PathBuf> {
    validate_tree_id(tree_id)?;
    ensure_state_dir()?;
    Ok(state_dir().join(format!("{tree_id}.lock")))
}

// ─────────────────────────────────────────────────────────────────────────────
// File-based locking (O_EXCL cross-process mutex with stale PID detection)
// ─────────────────────────────────────────────────────────────────────────────

struct FileLock {
    path: PathBuf,
}

impl FileLock {
    fn acquire(tree_id: &str) -> Result<Self> {
        let path = lock_path(tree_id)?;
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs_f64(LOCK_TIMEOUT_SECS);
        let pid = std::process::id();

        loop {
            // Try O_CREAT | O_EXCL | O_WRONLY
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = write!(f, "{pid}");
                    return Ok(FileLock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Check if lock holder is alive
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(holder_pid) = content.trim().parse::<i32>() {
                            // kill(pid, 0) checks if process exists
                            let alive =
                                unsafe { libc::kill(holder_pid, 0) } == 0;
                            if !alive {
                                // Stale lock — remove it
                                let _ = fs::remove_file(&path);
                            }
                        }
                        // If content is not a valid PID, just wait
                    }
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("Failed to create lock file: {}", path.display())
                    });
                }
            }

            if std::time::Instant::now() >= deadline {
                bail!(
                    "Could not acquire file lock for tree {:?} within {LOCK_TIMEOUT_SECS}s",
                    tree_id
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(LOCK_SPIN_MS));
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State I/O
// ─────────────────────────────────────────────────────────────────────────────

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn load_state(tree_id: &str) -> TreeState {
    let path = match state_path(tree_id) {
        Ok(p) => p,
        Err(_) => return TreeState::default(),
    };
    if !path.exists() {
        return TreeState::default();
    }
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<TreeState>(&text) {
            Ok(state) => state,
            Err(e) => {
                eprintln!(
                    "WARNING: session_tree: corrupted state for {:?}: {e}",
                    tree_id
                );
                TreeState::default()
            }
        },
        Err(e) => {
            eprintln!(
                "WARNING: session_tree: corrupted state for {:?}: {e}",
                tree_id
            );
            TreeState::default()
        }
    }
}

fn save_state(tree_id: &str, state: &mut TreeState) -> Result<()> {
    let now = now_epoch();
    let completed_cutoff = now - (COMPLETED_MAX_AGE_HOURS * 3600.0);
    let active_cutoff = now - (ACTIVE_MAX_AGE_HOURS * 3600.0);

    // Prune stale sessions
    state.sessions.retain(|sid, s| {
        if s.status == "completed" && s.completed_at < completed_cutoff {
            return false;
        }
        if s.status == "active" && s.started_at < active_cutoff {
            let age_hours = (now - s.started_at) / 3600.0;
            eprintln!(
                "WARNING: session_tree: pruning leaked active session {:?} (started {age_hours:.1}h ago)",
                sid
            );
            return false;
        }
        true
    });

    // Atomic write via temp file + rename
    let target = state_path(tree_id)?;
    let dir = state_dir();
    let content = serde_json::to_string_pretty(state)?;

    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("Failed to create temp file in {}", dir.display()))?;
    fs::write(tmp.path(), &content)?;
    // persist() does rename
    tmp.persist(&target)
        .with_context(|| format!("Failed to persist state to {}", target.display()))?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ID generation (replaces uuid.uuid4().hex[:8])
// ─────────────────────────────────────────────────────────────────────────────

fn generate_short_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    let mut hasher = Sha256::new();
    hasher.update(now.as_nanos().to_le_bytes());
    hasher.update(pid.to_le_bytes());
    // Mix in a pointer to a stack variable for extra entropy
    let stack_var: u8 = 0;
    let addr = &stack_var as *const u8 as usize;
    hasher.update(addr.to_le_bytes());
    let hash = hasher.finalize();
    // 8 hex chars (4 bytes)
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Core operations
// ─────────────────────────────────────────────────────────────────────────────

fn check_can_spawn() -> (bool, String, u32, u32) {
    let ctx = get_tree_context();
    let tree_id = &ctx.tree_id;
    let depth = ctx.depth;
    let max_depth = ctx.max_depth;
    let max_sessions = ctx.max_sessions;
    let child_depth = depth + 1;

    if child_depth > max_depth {
        return (
            false,
            format!("max_depth={max_depth} exceeded at depth={depth}"),
            0,
            depth,
        );
    }

    if tree_id.is_empty() {
        // Root session creating first tree — always allowed
        return (true, "new_tree".to_string(), 0, 0);
    }

    if validate_tree_id(tree_id).is_err() {
        return (false, "invalid_tree_id".to_string(), 0, depth);
    }

    let state = load_state(tree_id);
    let active_count = state
        .sessions
        .values()
        .filter(|s| s.status == "active")
        .count() as u32;

    if active_count >= max_sessions {
        return (
            false,
            format!("max_sessions={max_sessions} reached ({active_count} active)"),
            active_count,
            depth,
        );
    }

    (true, "ok".to_string(), active_count, depth)
}

fn register_session(
    session_id: &str,
    parent_id: Option<&str>,
) -> Result<(String, u32)> {
    let ctx = get_tree_context();
    let tree_id = if ctx.tree_id.is_empty() {
        generate_short_id()
    } else {
        ctx.tree_id.clone()
    };
    validate_tree_id(&tree_id)?;
    let depth = ctx.depth;
    let max_sessions = ctx.max_sessions;
    let max_depth = ctx.max_depth;

    let _lock = FileLock::acquire(&tree_id)?;
    let mut state = load_state(&tree_id);

    // Atomic capacity and depth check
    let active_count = state
        .sessions
        .values()
        .filter(|s| s.status == "active")
        .count() as u32;

    if active_count >= max_sessions {
        bail!(
            "max_sessions={max_sessions} reached ({active_count} active)"
        );
    }
    if depth > max_depth {
        bail!("depth={depth} exceeds max_depth={max_depth}");
    }

    let entry = SessionEntry {
        depth,
        parent: parent_id.map(|s| s.to_string()),
        status: "active".to_string(),
        started_at: now_epoch(),
        completed_at: 0.0,
        children: Vec::new(),
    };
    state
        .sessions
        .insert(session_id.to_string(), entry);

    if let Some(pid) = parent_id {
        if let Some(parent) = state.sessions.get_mut(pid) {
            parent.children.push(session_id.to_string());
        }
    }

    save_state(&tree_id, &mut state)?;
    Ok((tree_id, depth))
}

fn complete_session(session_id: &str) -> Result<()> {
    let ctx = get_tree_context();
    let tree_id = &ctx.tree_id;
    if tree_id.is_empty() {
        return Ok(());
    }
    validate_tree_id(tree_id)?;

    let _lock = FileLock::acquire(tree_id)?;
    let mut state = load_state(tree_id);

    if let Some(session) = state.sessions.get_mut(session_id) {
        session.status = "completed".to_string();
        session.completed_at = now_epoch();
    }

    save_state(tree_id, &mut state)?;
    Ok(())
}

fn get_status(tree_id: &str) -> Result<serde_json::Value> {
    validate_tree_id(tree_id)?;
    let state = load_state(tree_id);

    let active: Vec<&String> = state
        .sessions
        .iter()
        .filter(|(_, s)| s.status == "active")
        .map(|(sid, _)| sid)
        .collect();
    let completed: Vec<&String> = state
        .sessions
        .iter()
        .filter(|(_, s)| s.status == "completed")
        .map(|(sid, _)| sid)
        .collect();
    let depths: HashMap<&String, u32> = state
        .sessions
        .iter()
        .map(|(sid, s)| (sid, s.depth))
        .collect();

    Ok(serde_json::json!({
        "tree_id": tree_id,
        "active": active,
        "completed": completed,
        "depths": depths,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI dispatch
// ─────────────────────────────────────────────────────────────────────────────

pub fn run_session_tree(command: SessionTreeCommands) -> Result<()> {
    match command {
        SessionTreeCommands::Check => {
            let (allowed, reason, _active, _depth) = check_can_spawn();
            if allowed {
                println!("ALLOWED");
            } else {
                println!("BLOCKED:{reason}");
            }
            Ok(())
        }
        SessionTreeCommands::Register {
            session_id,
            parent_id,
        } => {
            match register_session(&session_id, parent_id.as_deref()) {
                Ok((tree_id, depth)) => {
                    println!("TREE_ID={tree_id} DEPTH={depth}");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                }
            }
        }
        SessionTreeCommands::Complete { session_id } => {
            complete_session(&session_id)?;
            Ok(())
        }
        SessionTreeCommands::Status { tree_id } => {
            let tid = tree_id.unwrap_or_else(|| {
                std::env::var("AMPLIHACK_TREE_ID").unwrap_or_default()
            });
            if tid.is_empty() {
                eprintln!("No AMPLIHACK_TREE_ID set");
                std::process::exit(1);
            }
            let status = get_status(&tid)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
    }
}
