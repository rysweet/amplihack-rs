//! Session tree management for amplihack orchestration.
//!
//! Prevents infinite recursion by tracking active sessions in a tree structure.
//! Enforces max depth and max concurrent session limits.
//!
//! State file: `/tmp/amplihack-session-trees/{tree_id}.json`
//! Lock file:  `/tmp/amplihack-session-trees/{tree_id}.lock`

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use std::{fs, io};

use crate::command_error::exit_error;

// Compile once.
static TREE_ID_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").unwrap());

const DEFAULT_MAX_DEPTH: u32 = 3;
const DEFAULT_MAX_SESSIONS: u32 = 10;
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const COMPLETED_MAX_AGE_HOURS: f64 = 24.0;
const ACTIVE_MAX_AGE_HOURS: f64 = 4.0;
const STALE_LOCK_AGE: Duration = Duration::from_secs(10 * 60);

// ─── Types ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct TreeState {
    sessions: HashMap<String, SessionEntry>,
    #[serde(default)]
    spawn_count: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SessionEntry {
    depth: u32,
    parent: Option<String>,
    status: String,
    started_at: f64,
    #[serde(default)]
    completed_at: Option<f64>,
    #[serde(default)]
    children: Vec<String>,
}

struct TreeContext {
    tree_id: String,
    depth: u32,
    max_depth: u32,
    max_sessions: u32,
}

// ─── Validation & paths ──────────────────────────────────────────────────

fn validate_tree_id(id: &str) -> Result<&str> {
    if id.is_empty() {
        bail!("tree_id cannot be empty");
    }
    if !TREE_ID_RE.is_match(id) {
        bail!("Invalid tree_id {id:?}: must match [a-zA-Z0-9_-]{{1,64}}");
    }
    Ok(id)
}

fn state_dir() -> PathBuf {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(tmp).join("amplihack-session-trees")
}

fn ensure_state_dir() -> Result<()> {
    let dir = state_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn state_path(tree_id: &str) -> Result<PathBuf> {
    validate_tree_id(tree_id)?;
    let dir = state_dir();
    let candidate = dir.join(format!("{tree_id}.json"));
    let resolved = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    let dir_resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    if !resolved.starts_with(&dir_resolved) && candidate.exists() {
        bail!("Path traversal detected for tree_id {tree_id:?}");
    }
    Ok(dir.join(format!("{tree_id}.json")))
}

fn lock_path(tree_id: &str) -> PathBuf {
    state_dir().join(format!("{tree_id}.lock"))
}

// ─── Environment helpers ─────────────────────────────────────────────────

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
    TreeContext {
        tree_id: std::env::var("AMPLIHACK_TREE_ID").unwrap_or_default(),
        depth,
        max_depth,
        max_sessions,
    }
}

// ─── File I/O with locking ───────────────────────────────────────────────

fn with_lock<F, R>(tree_id: &str, f: F) -> Result<R>
where
    F: FnOnce() -> Result<R>,
{
    ensure_state_dir()?;
    let lock = lock_path(tree_id);
    let deadline = Instant::now() + LOCK_TIMEOUT;

    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(mut file) => {
                let _ = write!(file, "{}", std::process::id());
                let result = f();
                let _ = fs::remove_file(&lock);
                return result;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if Instant::now() >= deadline {
                    bail!(
                        "Could not acquire file lock for tree {tree_id:?} within {}s",
                        LOCK_TIMEOUT.as_secs()
                    );
                }
                // Check lock age, then PID liveness
                let lock_age = fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok());

                if let Some(age) = lock_age {
                    if age > STALE_LOCK_AGE {
                        eprintln!(
                            "WARNING: session_tree: removing stale lock for {tree_id:?} \
                             (age: {:.0}s > {}s threshold)",
                            age.as_secs_f64(),
                            STALE_LOCK_AGE.as_secs()
                        );
                        let _ = fs::remove_file(&lock);
                    } else if let Ok(content) = fs::read_to_string(&lock)
                        && let Ok(pid) = content.trim().parse::<i32>()
                    {
                        #[cfg(unix)]
                        {
                            if unsafe { libc::kill(pid, 0) } != 0 {
                                let _ = fs::remove_file(&lock);
                            }
                        }
                        #[cfg(not(unix))]
                        let _ = pid;
                    }
                }
                std::thread::sleep(LOCK_POLL_INTERVAL);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn load(tree_id: &str) -> TreeState {
    let p = match state_path(tree_id) {
        Ok(p) => p,
        Err(_) => return TreeState::default(),
    };
    if !p.exists() {
        return TreeState::default();
    }
    match fs::read_to_string(&p) {
        Ok(content) => match serde_json::from_str::<TreeState>(&content) {
            Ok(state) => state,
            Err(e) => {
                eprintln!("WARNING: session_tree: corrupted state for {tree_id:?}: {e}");
                TreeState::default()
            }
        },
        Err(e) => {
            eprintln!("WARNING: session_tree: corrupted state for {tree_id:?}: {e}");
            TreeState::default()
        }
    }
}

fn save(tree_id: &str, state: &mut TreeState) -> Result<()> {
    let now = now_epoch();
    let completed_cutoff = now - (COMPLETED_MAX_AGE_HOURS * 3600.0);
    let active_cutoff = now - (ACTIVE_MAX_AGE_HOURS * 3600.0);

    // Prune stale sessions
    state.sessions.retain(|sid, s| {
        if s.status == "completed" && s.completed_at.unwrap_or(0.0) < completed_cutoff {
            return false;
        }
        if s.status == "active" && s.started_at < active_cutoff {
            eprintln!(
                "WARNING: session_tree: pruning leaked active session {sid:?} \
                 (started {:.1}h ago)",
                (now - s.started_at) / 3600.0
            );
            return false;
        }
        true
    });

    // Atomic write via temp file + rename
    let target = state_path(tree_id)?;
    let dir = state_dir();
    let content = serde_json::to_string_pretty(state)?;
    let tmp = tempfile::NamedTempFile::new_in(&dir)?;
    fs::write(tmp.path(), &content)?;
    tmp.persist(&target)?;

    Ok(())
}

fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn gen_hex_id() -> String {
    // Use system time + pid for uniqueness without requiring rand crate
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    format!("{:08x}", (ts as u32) ^ pid)
}

// ─── Core operations ─────────────────────────────────────────────────────

struct CheckResult {
    allowed: bool,
    reason: String,
}

fn check_can_spawn() -> CheckResult {
    let ctx = get_tree_context();
    let child_depth = ctx.depth + 1;

    if child_depth > ctx.max_depth {
        return CheckResult {
            allowed: false,
            reason: format!(
                "max_depth={} exceeded at depth={}",
                ctx.max_depth, ctx.depth
            ),
        };
    }

    if ctx.tree_id.is_empty() {
        return CheckResult {
            allowed: true,
            reason: "new_tree".to_string(),
        };
    }

    if validate_tree_id(&ctx.tree_id).is_err() {
        return CheckResult {
            allowed: false,
            reason: format!("invalid tree_id: {:?}", ctx.tree_id),
        };
    }

    let state = load(&ctx.tree_id);
    let active_count = state
        .sessions
        .values()
        .filter(|s| s.status == "active")
        .count() as u32;

    if active_count >= ctx.max_sessions {
        return CheckResult {
            allowed: false,
            reason: format!(
                "max_sessions={} reached ({active_count} active)",
                ctx.max_sessions
            ),
        };
    }

    CheckResult {
        allowed: true,
        reason: "ok".to_string(),
    }
}

fn register_session(session_id: &str, parent_id: Option<&str>) -> Result<(String, u32)> {
    let ctx = get_tree_context();
    let tree_id_raw = if ctx.tree_id.is_empty() {
        gen_hex_id()
    } else {
        ctx.tree_id.clone()
    };
    let tree_id = validate_tree_id(&tree_id_raw)?.to_string();
    let depth = ctx.depth;
    let max_sessions = ctx.max_sessions;
    let max_depth = ctx.max_depth;

    with_lock(&tree_id, || {
        let mut state = load(&tree_id);

        let active_count = state
            .sessions
            .values()
            .filter(|s| s.status == "active")
            .count() as u32;

        if active_count >= max_sessions {
            bail!("max_sessions={max_sessions} reached ({active_count} active)");
        }
        if depth > max_depth {
            bail!("depth={depth} exceeds max_depth={max_depth}");
        }

        state.sessions.insert(
            session_id.to_string(),
            SessionEntry {
                depth,
                parent: parent_id.map(String::from),
                status: "active".to_string(),
                started_at: now_epoch(),
                completed_at: None,
                children: Vec::new(),
            },
        );

        if let Some(pid) = parent_id
            && let Some(parent) = state.sessions.get_mut(pid)
        {
            parent.children.push(session_id.to_string());
        }

        save(&tree_id, &mut state)?;
        Ok(())
    })?;

    Ok((tree_id, depth))
}

fn complete_session_impl(session_id: &str) -> Result<()> {
    let ctx = get_tree_context();
    if ctx.tree_id.is_empty() {
        return Ok(());
    }

    let tree_id = ctx.tree_id;
    validate_tree_id(&tree_id)?;

    with_lock(&tree_id, || {
        let mut state = load(&tree_id);
        if let Some(session) = state.sessions.get_mut(session_id) {
            session.status = "completed".to_string();
            session.completed_at = Some(now_epoch());
        }
        save(&tree_id, &mut state)?;
        Ok(())
    })
}

fn get_status_impl(tree_id: &str) -> Result<serde_json::Value> {
    validate_tree_id(tree_id)?;
    let state = load(tree_id);

    let active: Vec<&String> = state
        .sessions
        .iter()
        .filter(|(_, s)| s.status == "active")
        .map(|(k, _)| k)
        .collect();
    let completed: Vec<&String> = state
        .sessions
        .iter()
        .filter(|(_, s)| s.status == "completed")
        .map(|(k, _)| k)
        .collect();
    let depths: HashMap<&String, u32> = state.sessions.iter().map(|(k, s)| (k, s.depth)).collect();

    Ok(serde_json::json!({
        "tree_id": tree_id,
        "active": active,
        "completed": completed,
        "depths": depths,
    }))
}

// ─── CLI entry points ────────────────────────────────────────────────────

pub fn run_check() -> Result<()> {
    let result = check_can_spawn();
    if result.allowed {
        println!("ALLOWED");
    } else {
        println!("BLOCKED:{}", result.reason);
    }
    Ok(())
}

pub fn run_register(session_id: Option<String>, parent_id: Option<String>) -> Result<()> {
    let sid = session_id.unwrap_or_else(gen_hex_id);
    match register_session(&sid, parent_id.as_deref()) {
        Ok((tree_id, depth)) => {
            println!("TREE_ID={tree_id} DEPTH={depth}");
            Ok(())
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            Err(exit_error(1))
        }
    }
}

pub fn run_complete(session_id: Option<String>) -> Result<()> {
    let sid = session_id.unwrap_or_default();
    complete_session_impl(&sid)
}

pub fn run_status(tree_id: Option<String>) -> Result<()> {
    let ctx = get_tree_context();
    let tid = tree_id.unwrap_or(ctx.tree_id);
    if tid.is_empty() {
        println!("No AMPLIHACK_TREE_ID set");
        return Err(exit_error(1));
    }
    let status = get_status_impl(&tid)?;
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(vars: &[(&str, Option<&str>)]) -> Self {
            let saved: Vec<_> = vars
                .iter()
                .map(|(k, v)| {
                    let old = std::env::var(k).ok();
                    match v {
                        Some(val) => unsafe { std::env::set_var(k, val) },
                        None => unsafe { std::env::remove_var(k) },
                    }
                    (k.to_string(), old)
                })
                .collect();
            Self { vars: saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.vars {
                match v {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    fn unique_tree_id(test_name: &str) -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("test-{test_name}-{ts}")
    }

    fn cleanup_tree(tree_id: &str) {
        let dir = state_dir();
        let _ = fs::remove_file(dir.join(format!("{tree_id}.json")));
        let _ = fs::remove_file(dir.join(format!("{tree_id}.lock")));
    }

    #[test]
    fn validate_tree_id_accepts_valid_ids() {
        assert!(validate_tree_id("abc123").is_ok());
        assert!(validate_tree_id("my-tree_01").is_ok());
        assert!(validate_tree_id("A").is_ok());
        assert!(validate_tree_id(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_tree_id_rejects_empty() {
        assert!(validate_tree_id("").is_err());
    }

    #[test]
    fn validate_tree_id_rejects_path_traversal() {
        assert!(validate_tree_id("../etc/passwd").is_err());
    }

    #[test]
    fn validate_tree_id_rejects_special_chars() {
        assert!(validate_tree_id("tree;rm -rf /").is_err());
        assert!(validate_tree_id("tree/sub").is_err());
        assert!(validate_tree_id("tree.json").is_err());
    }

    #[test]
    fn gen_hex_id_produces_8_hex_chars() {
        let id = gen_hex_id();
        assert_eq!(id.len(), 8, "Expected 8 chars, got {}: {id}", id.len());
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "Expected hex chars: {id}"
        );
    }

    #[test]
    fn check_allows_when_no_tree() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", None),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);
        let result = check_can_spawn();
        assert!(result.allowed, "Should be allowed: {}", result.reason);
        assert_eq!(result.reason, "new_tree");
    }

    #[test]
    fn check_blocks_at_max_depth() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", Some("any")),
            ("AMPLIHACK_SESSION_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);
        let result = check_can_spawn();
        assert!(!result.allowed);
        assert!(result.reason.contains("max_depth"));
    }

    #[test]
    fn register_creates_new_tree_when_id_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", None),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);

        let (tree_id, depth) = register_session("test-sess", None).unwrap();
        assert_eq!(depth, 0);
        assert_eq!(tree_id.len(), 8);

        let state = load(&tree_id);
        assert!(state.sessions.contains_key("test-sess"));
        assert_eq!(state.sessions["test-sess"].status, "active");
        cleanup_tree(&tree_id);
    }

    #[test]
    fn register_with_parent_links_child() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tid = unique_tree_id("reg-parent");
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", Some(&tid)),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);

        register_session("parent-sess", None).unwrap();
        register_session("child-sess", Some("parent-sess")).unwrap();

        let state = load(&tid);
        assert!(
            state.sessions["parent-sess"]
                .children
                .contains(&"child-sess".to_string())
        );
        cleanup_tree(&tid);
    }

    #[test]
    fn register_fails_when_max_sessions_reached() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tid = unique_tree_id("reg-maxsess");
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", Some(&tid)),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("2")),
        ]);

        register_session("s1", None).unwrap();
        register_session("s2", None).unwrap();
        let result = register_session("s3", None);
        assert!(result.is_err(), "Should fail at max sessions");
        cleanup_tree(&tid);
    }

    #[test]
    fn complete_marks_session_completed() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tid = unique_tree_id("complete-ok");
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", Some(&tid)),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);

        register_session("my-sess", None).unwrap();
        complete_session_impl("my-sess").unwrap();

        let state = load(&tid);
        assert_eq!(state.sessions["my-sess"].status, "completed");
        assert!(state.sessions["my-sess"].completed_at.is_some());
        cleanup_tree(&tid);
    }

    #[test]
    fn status_returns_tree_details() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tid = unique_tree_id("status-ok");
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", Some(&tid)),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);

        register_session("s1", None).unwrap();
        register_session("s2", None).unwrap();
        complete_session_impl("s1").unwrap();

        let status = get_status_impl(&tid).unwrap();
        assert_eq!(status["tree_id"], tid);

        let active = status["active"].as_array().unwrap();
        let completed = status["completed"].as_array().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(completed.len(), 1);
        cleanup_tree(&tid);
    }

    #[test]
    fn full_lifecycle_register_complete_status() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tid = unique_tree_id("lifecycle");
        let _guard = EnvGuard::new(&[
            ("AMPLIHACK_TREE_ID", Some(&tid)),
            ("AMPLIHACK_SESSION_DEPTH", Some("0")),
            ("AMPLIHACK_MAX_DEPTH", Some("3")),
            ("AMPLIHACK_MAX_SESSIONS", Some("10")),
        ]);

        let (tree_id, depth) = register_session("root", None).unwrap();
        assert_eq!(tree_id, tid);
        assert_eq!(depth, 0);

        register_session("child", Some("root")).unwrap();
        complete_session_impl("child").unwrap();
        complete_session_impl("root").unwrap();

        let status = get_status_impl(&tid).unwrap();
        assert_eq!(status["completed"].as_array().unwrap().len(), 2);
        cleanup_tree(&tid);
    }
}
