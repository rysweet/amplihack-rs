//! On-disk session-tree state management.
//!
//! The session-tree state is kept under `$TMPDIR/amplihack-session-trees/`,
//! one JSON file per tree (`{tree_id}.json`). Concurrent access is serialised
//! via two layers of locking:
//!
//! * an intra-process [`std::sync::Mutex`] keyed by tree_id, so threads in
//!   the same process cannot race each other while holding the file lock
//!   (file lock semantics on POSIX are per-process, not per-thread)
//! * a cross-process exclusive file lock acquired via [`fs4`] on a sidecar
//!   `{tree_id}.lock` file
//!
//! All state writes go through `with_locked_tree` to guarantee atomic
//! check-then-write semantics. State files are written atomically via
//! tmpfile + rename.

use std::collections::HashMap;
#[cfg(unix)]
use std::fs::Permissions;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use fs4::fs_std::FileExt;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Directory name (under `TMPDIR` or `/tmp`) where tree state files live.
const STATE_DIR_NAME: &str = "amplihack-session-trees";

/// Maximum permitted size of a single tree state file (1 MiB). Protects
/// against pathological growth or a malicious actor planting a huge file
/// before we can read it.
const MAX_STATE_FILE_BYTES: u64 = 1024 * 1024;

/// Default maximum recursion depth (root = depth 0).
pub const DEFAULT_MAX_DEPTH: u32 = 3;

/// Default cap on simultaneously-active sessions per tree.
pub const DEFAULT_MAX_SESSIONS: u32 = 10;

/// Hard ceiling on `max_depth` to prevent fork-bomb-style misconfiguration.
pub const MAX_DEPTH_CEILING: u32 = 32;

/// Stale-pruning thresholds (matches the original Python contract).
const COMPLETED_MAX_AGE_SECS: u64 = 24 * 60 * 60;
const ACTIVE_MAX_AGE_SECS: u64 = 4 * 60 * 60;

fn tree_id_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").expect("tree_id regex compiles"))
}

/// Validate a tree_id (or session_id, which uses the same alphabet).
///
/// Returns the input on success. Rejects anything that could be used for
/// path traversal or shell injection.
pub fn validate_tree_id(id: &str) -> Result<&str> {
    if id.is_empty() {
        bail!("tree_id cannot be empty");
    }
    if !tree_id_regex().is_match(id) {
        bail!("invalid tree_id {id:?}: must match [A-Za-z0-9_-]{{1,64}}");
    }
    Ok(id)
}

/// Resolve the state directory, creating it with mode 0700 if necessary.
pub fn state_dir() -> Result<PathBuf> {
    // `AMPLIHACK_SESSION_TREE_DIR` is an internal override (used by tests and
    // anyone who wants to relocate state without disturbing the global TMPDIR).
    // Production callers should leave it unset; the recipe reads/writes via
    // the same `amplihack session-tree` binary, so the env-var contract stays
    // self-consistent across parent and child processes.
    let dir = if let Some(explicit) = std::env::var_os("AMPLIHACK_SESSION_TREE_DIR") {
        PathBuf::from(explicit)
    } else {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        base.join(STATE_DIR_NAME)
    };
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create state dir {}", dir.display()))?;
    #[cfg(unix)]
    {
        // Best-effort 0700; ignore if we don't own the directory.
        let _ = fs::set_permissions(&dir, Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// Compute the path to a tree's state file under the given directory.
///
/// Validates the tree_id and verifies that the resolved path stays under
/// `dir` (defence-in-depth — the regex already excludes `/` and `..`).
pub fn state_path_in(dir: &Path, tree_id: &str) -> Result<PathBuf> {
    validate_tree_id(tree_id)?;
    let candidate = dir.join(format!("{tree_id}.json"));
    let canonical_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if let Some(parent) = candidate.parent() {
        let canonical_parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        if canonical_parent != canonical_dir {
            bail!("path traversal detected for tree_id {tree_id:?}");
        }
    }
    Ok(candidate)
}

fn lock_path_in(dir: &Path, tree_id: &str) -> Result<PathBuf> {
    validate_tree_id(tree_id)?;
    Ok(dir.join(format!("{tree_id}.lock")))
}

/// Status of a single session within a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Completed,
}

/// One session entry in the tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub depth: u32,
    pub parent: Option<String>,
    pub status: SessionStatus,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
    #[serde(default)]
    pub children: Vec<String>,
}

/// Full on-disk shape of a tree's state file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreeState {
    #[serde(default)]
    pub sessions: HashMap<String, SessionEntry>,
}

impl TreeState {
    pub fn active_count(&self) -> u32 {
        self.sessions
            .values()
            .filter(|s| s.status == SessionStatus::Active)
            .count() as u32
    }

    pub fn active_ids(&self) -> Vec<String> {
        let mut v: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.status == SessionStatus::Active)
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }

    pub fn completed_ids(&self) -> Vec<String> {
        let mut v: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.status == SessionStatus::Completed)
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load (or initialise) the state for one tree. Caps the read at 1 MiB and
/// degrades to an empty state on JSON parse failure (logged via tracing).
pub fn load_state(path: &Path) -> Result<TreeState> {
    if !path.exists() {
        return Ok(TreeState::default());
    }
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > MAX_STATE_FILE_BYTES {
        tracing::warn!(
            path = %path.display(),
            len = metadata.len(),
            "session_tree state file exceeds {MAX_STATE_FILE_BYTES} bytes; treating as empty"
        );
        return Ok(TreeState::default());
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        // O_NOFOLLOW + O_CLOEXEC defeat symlink TOCTOU on shared /tmp.
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .with_context(|| format!("failed to read {}", path.display()))?;
    match serde_json::from_str::<TreeState>(&buf) {
        Ok(state) => Ok(state),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "session_tree state file is corrupt; treating as empty"
            );
            Ok(TreeState::default())
        }
    }
}

/// Atomically write state to disk via tmpfile + rename. Sets mode 0600 on
/// the destination (Unix). Prunes stale completed and leaked-active
/// sessions before writing.
pub fn save_state(path: &Path, mut state: TreeState) -> Result<()> {
    prune_stale(&mut state);

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("state path has no parent: {}", path.display()))?;
    let file_stem = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("state path has no file name: {}", path.display()))?;
    let tmp = parent.join(format!(".{file_stem}.tmp.{}", std::process::id()));

    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        opts.mode(0o600);
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let payload =
        serde_json::to_string_pretty(&state).with_context(|| "failed to serialise tree state")?;
    {
        let mut tmp_file = opts
            .open(&tmp)
            .with_context(|| format!("failed to create temp file {}", tmp.display()))?;
        tmp_file
            .write_all(payload.as_bytes())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        tmp_file
            .sync_all()
            .with_context(|| format!("failed to fsync {}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn prune_stale(state: &mut TreeState) {
    let now = now_secs();
    let completed_cutoff = now.saturating_sub(COMPLETED_MAX_AGE_SECS);
    let active_cutoff = now.saturating_sub(ACTIVE_MAX_AGE_SECS);
    state.sessions.retain(|sid, entry| match entry.status {
        SessionStatus::Completed => {
            let when = entry.completed_at.unwrap_or(0);
            if when < completed_cutoff {
                tracing::debug!(session = %sid, "pruning stale completed session");
                false
            } else {
                true
            }
        }
        SessionStatus::Active => {
            if entry.started_at < active_cutoff {
                tracing::warn!(
                    session = %sid,
                    "pruning leaked active session (started {} secs ago)",
                    now.saturating_sub(entry.started_at)
                );
                false
            } else {
                true
            }
        }
    });
}

fn process_lock_for(tree_id: &str) -> &'static Mutex<()> {
    use std::sync::Mutex as StdMutex;
    static LOCKS: OnceLock<StdMutex<HashMap<String, &'static Mutex<()>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut guard = map.lock().expect("session_tree process-lock map poisoned");
    if let Some(existing) = guard.get(tree_id) {
        return existing;
    }
    let leaked: &'static Mutex<()> = Box::leak(Box::new(Mutex::new(())));
    guard.insert(tree_id.to_string(), leaked);
    leaked
}

/// Run `op` while holding both an intra-process lock and a cross-process
/// exclusive file lock for `tree_id`. The lock is released when this
/// function returns.
pub fn with_locked_tree<F, T>(dir: &Path, tree_id: &str, op: F) -> Result<T>
where
    F: FnOnce(&Path) -> Result<T>,
{
    validate_tree_id(tree_id)?;
    let _process_guard = process_lock_for(tree_id)
        .lock()
        .map_err(|_| anyhow!("intra-process lock for tree {tree_id:?} poisoned"))?;

    let lock_path = lock_path_in(dir, tree_id)?;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(false).read(true);
    #[cfg(unix)]
    {
        opts.mode(0o600);
        opts.custom_flags(libc::O_CLOEXEC);
    }
    let lock_file: File = opts
        .open(&lock_path)
        .with_context(|| format!("failed to open lock file {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock_file)
        .with_context(|| format!("failed to acquire lock on {}", lock_path.display()))?;

    let result = op(&state_path_in(dir, tree_id)?);

    // Best-effort unlock; the kernel will release on drop regardless.
    let _ = FileExt::unlock(&lock_file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use tempfile::TempDir;

    fn temp_state_dir() -> (TempDir, PathBuf) {
        let td = TempDir::new_in("/tmp").expect("tempdir");
        let p = td.path().to_path_buf();
        (td, p)
    }

    #[test]
    fn validate_accepts_valid_ids() {
        assert!(validate_tree_id("abc123").is_ok());
        assert!(validate_tree_id("AB_cd-EF").is_ok());
        assert!(validate_tree_id(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn validate_rejects_path_traversal() {
        assert!(validate_tree_id("").is_err());
        assert!(validate_tree_id("..").is_err());
        assert!(validate_tree_id("a/b").is_err());
        assert!(validate_tree_id("a.b").is_err());
        assert!(validate_tree_id(&"a".repeat(65)).is_err());
        assert!(validate_tree_id("../etc/passwd").is_err());
    }

    #[test]
    fn state_path_lives_under_dir() {
        let (_td, dir) = temp_state_dir();
        let p = state_path_in(&dir, "tree1").unwrap();
        assert_eq!(p, dir.join("tree1.json"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let (_td, dir) = temp_state_dir();
        let path = state_path_in(&dir, "rt").unwrap();
        let mut state = TreeState::default();
        state.sessions.insert(
            "s1".into(),
            SessionEntry {
                depth: 0,
                parent: None,
                status: SessionStatus::Active,
                started_at: now_secs(),
                completed_at: None,
                children: vec![],
            },
        );
        save_state(&path, state).unwrap();
        let loaded = load_state(&path).unwrap();
        assert!(loaded.sessions.contains_key("s1"));
        assert_eq!(loaded.active_count(), 1);
    }

    #[test]
    fn load_missing_returns_default() {
        let (_td, dir) = temp_state_dir();
        let path = dir.join("missing.json");
        let state = load_state(&path).unwrap();
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn load_corrupt_returns_default() {
        let (_td, dir) = temp_state_dir();
        let path = state_path_in(&dir, "corrupt").unwrap();
        fs::write(&path, "not json{{{").unwrap();
        let state = load_state(&path).unwrap();
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn load_oversized_file_returns_default() {
        let (_td, dir) = temp_state_dir();
        let path = state_path_in(&dir, "big").unwrap();
        let big = "x".repeat((MAX_STATE_FILE_BYTES + 1) as usize);
        fs::write(&path, big).unwrap();
        let state = load_state(&path).unwrap();
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn save_writes_atomically_no_temp_leftover() {
        let (_td, dir) = temp_state_dir();
        let path = state_path_in(&dir, "atomic").unwrap();
        save_state(&path, TreeState::default()).unwrap();
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "found temp leftovers: {leftovers:?}");
    }

    #[test]
    fn prune_drops_stale_completed_and_leaked_active() {
        let mut state = TreeState::default();
        let stale_completed_at = now_secs().saturating_sub(COMPLETED_MAX_AGE_SECS + 60);
        let stale_active_at = now_secs().saturating_sub(ACTIVE_MAX_AGE_SECS + 60);
        state.sessions.insert(
            "stale_completed".into(),
            SessionEntry {
                depth: 0,
                parent: None,
                status: SessionStatus::Completed,
                started_at: stale_completed_at,
                completed_at: Some(stale_completed_at),
                children: vec![],
            },
        );
        state.sessions.insert(
            "stale_active".into(),
            SessionEntry {
                depth: 0,
                parent: None,
                status: SessionStatus::Active,
                started_at: stale_active_at,
                completed_at: None,
                children: vec![],
            },
        );
        state.sessions.insert(
            "fresh".into(),
            SessionEntry {
                depth: 0,
                parent: None,
                status: SessionStatus::Active,
                started_at: now_secs(),
                completed_at: None,
                children: vec![],
            },
        );
        prune_stale(&mut state);
        assert_eq!(state.sessions.len(), 1);
        assert!(state.sessions.contains_key("fresh"));
    }

    #[test]
    fn concurrent_register_no_lost_updates() {
        // 16 threads each register one unique session under the same tree.
        // Lost updates would manifest as fewer than 16 sessions on disk.
        let (_td, dir) = temp_state_dir();
        let dir = Arc::new(dir);
        let mut handles = vec![];
        for i in 0..16 {
            let dir = Arc::clone(&dir);
            handles.push(thread::spawn(move || {
                with_locked_tree(&dir, "concur", |path| {
                    let mut state = load_state(path)?;
                    state.sessions.insert(
                        format!("s{i}"),
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
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let state = load_state(&state_path_in(&dir, "concur").unwrap()).unwrap();
        assert_eq!(
            state.sessions.len(),
            16,
            "expected 16 sessions, got {}",
            state.sessions.len()
        );
    }

    #[cfg(unix)]
    #[test]
    fn state_dir_has_restrictive_permissions() {
        let td = TempDir::new_in("/tmp").unwrap();
        // Set the session-tree-specific override; do NOT mutate global TMPDIR
        // because other parallel tests anchor `TempDir::new()` against it.
        unsafe {
            std::env::set_var("AMPLIHACK_SESSION_TREE_DIR", td.path().join("trees"));
        }
        let dir = state_dir().unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "state dir mode should be 0700, got {mode:o}");
        unsafe {
            std::env::remove_var("AMPLIHACK_SESSION_TREE_DIR");
        }
    }
}
