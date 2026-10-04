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

pub mod proofs;
pub mod state;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use state::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_SESSIONS, MAX_DEPTH_CEILING, SessionEntry, SessionStatus,
    effective_max_depth, load_state, save_state, state_dir, state_path_in, validate_tree_id,
    with_locked_tree,
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
        /// Emit a single-line JSON object `{"tree_id":..,"depth":..}` instead
        /// of the default `TREE_ID=.. DEPTH=..` text line. Additive/opt-in;
        /// lets consumers reuse the `orch helper extract-field` pipeline
        /// (issue #1062, finding D2).
        #[arg(long)]
        json: bool,
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
    /// Remove tree state older than the retention window (issue #1326).
    ///
    /// The store is durable now (it must be, or the tree-global cap counts
    /// nothing), so it needs an owner for its lifecycle. Previously it lived in
    /// TMPDIR and got free cleanup; that free cleanup is what made the cap
    /// meaningless, so it is not coming back.
    Gc {
        /// Delete trees whose last activity is older than this many days.
        #[arg(long, default_value_t = 7)]
        older_than_days: u64,
        /// Report what would be removed without removing it.
        #[arg(long)]
        dry_run: bool,
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
            json,
        } => run_register(ctx, session_id, parent_id, json),
        SessionTreeCommands::Complete { session_id } => run_complete(ctx, &session_id),
        SessionTreeCommands::Status { tree_id } => run_status(ctx, tree_id),
        SessionTreeCommands::Check => run_check(ctx),
        SessionTreeCommands::Gc {
            older_than_days,
            dry_run,
        } => run_gc(older_than_days, dry_run),
    }
}

fn run_register(
    ctx: TreeContext,
    session_id: Option<String>,
    parent_id: Option<String>,
    as_json: bool,
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
        // Issue #1326: the ceiling is sealed by the root registration and the
        // environment may only lower it thereafter, never raise it.
        let effective = effective_max_depth(state.ceiling, Some(max_depth));
        if state.ceiling.is_none() {
            state.ceiling = Some(effective);
        }
        if depth > effective {
            anyhow::bail!("depth={depth} exceeds max_depth={effective}");
        }
        let entry = SessionEntry {
            depth,
            parent: parent_id_clone.clone(),
            status: SessionStatus::Active,
            started_at: now_secs(),
            completed_at: None,
            children: vec![],
            pid: Some(std::process::id()),
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
            if as_json {
                // Opt-in single-line JSON; reuses the extract-field pipeline.
                println!("{}", json!({ "tree_id": tree_id, "depth": depth }));
            } else {
                // Byte-exact stdout contract consumed by smart-orchestrator.yaml.
                println!("TREE_ID={tree_id} DEPTH={depth}");
            }
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

/// `amplihack session-tree gc` — retention for the durable tree store (#1326).
fn run_gc(older_than_days: u64, dry_run: bool) -> Result<()> {
    let dir = state_dir()?;
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            older_than_days.saturating_mul(86_400),
        ))
        .unwrap_or(std::time::UNIX_EPOCH);

    let outcome = gc_in(&dir, cutoff, dry_run)?;

    println!(
        "{} {} file(s), {} bytes, older than {} day(s), from {}",
        if dry_run { "would remove" } else { "removed" },
        outcome.removed,
        outcome.bytes,
        older_than_days,
        dir.display()
    );
    if outcome.failed > 0 {
        // Exit non-zero so a caller that pipes this into automation notices.
        anyhow::bail!(
            "gc: {} file(s) could not be inspected or removed",
            outcome.failed
        );
    }
    Ok(())
}

/// What a garbage-collection pass actually did, as opposed to what it attempted.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GcOutcome {
    pub removed: usize,
    pub bytes: u64,
    pub failed: usize,
}

/// Retention pass over one tree-state directory (issue #1326).
///
/// Takes the directory and cutoff as arguments so the accounting is testable
/// without touching `$HOME` or process-global environment. Counts outcomes, never
/// intents: an earlier revision incremented before attempting the unlink, so a
/// read-only mount produced a summary claiming files were gone that were still on
/// disk.
pub(crate) fn gc_in(
    dir: &std::path::Path,
    cutoff: std::time::SystemTime,
    dry_run: bool,
) -> Result<GcOutcome> {
    let mut out = GcOutcome::default();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        // Report unreadable entries rather than skipping them. "removed 0 files"
        // must mean "there was nothing to remove", never "I could not look".
        let entry = match entry {
            Ok(e) => e,
            Err(error) => {
                eprintln!("gc: skipping unreadable directory entry: {error}");
                out.failed += 1;
                continue;
            }
        };
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("json") | Some("lock")
        ) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(error) => {
                eprintln!("gc: skipping {} (cannot stat): {error}", path.display());
                out.failed += 1;
                continue;
            }
        };
        if meta.modified().unwrap_or(std::time::UNIX_EPOCH) >= cutoff {
            continue;
        }
        if dry_run {
            println!("would remove {}", path.display());
            out.bytes += meta.len();
            out.removed += 1;
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                println!("removed {}", path.display());
                out.bytes += meta.len();
                out.removed += 1;
            }
            Err(error) => {
                eprintln!("gc: failed to remove {}: {error}", path.display());
                out.failed += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::state::TreeState;
    use super::*;
    use tempfile::TempDir;

    /// Every variable `isolated_env` takes ownership of for the length of a test.
    const TREE_ENV_KEYS: &[&str] = &[
        "AMPLIHACK_SESSION_TREE_DIR",
        "AMPLIHACK_TREE_ID",
        "AMPLIHACK_SESSION_DEPTH",
        "AMPLIHACK_MAX_DEPTH",
        "AMPLIHACK_MAX_SESSIONS",
    ];

    /// A private tree directory, the crate env lock, and the caller's previous
    /// environment — all released together, in that order, on drop.
    ///
    /// Issue #1380: this used to be handed back as a `(TempDir, MutexGuard)`
    /// pair, and the two bindings dropped independently. The lock was released
    /// while `AMPLIHACK_SESSION_TREE_DIR` still named the temp dir, and the temp
    /// dir was deleted immediately after — so the next test to take the lock
    /// inherited a tree directory that was already gone. That is how
    /// `failed to rename /tmp/.tmpXXXX/<hash>.json` (#1369) surfaced in tests
    /// that never touched the session tree. One guard makes the restore happen
    /// before either the lock or the directory can go away.
    struct IsolatedEnv {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _dir: TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for IsolatedEnv {
        fn drop(&mut self) {
            for (key, previous) in self.saved.drain(..) {
                match previous {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    fn isolated_env() -> IsolatedEnv {
        // Issue #1329/#1380: one lock for the whole crate. A module-private lock
        // does not serialise against tests in sibling modules that read the same
        // variables -- so a tree directory could be swapped out from under a test
        // that had "isolated" itself.
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // Anchor to /tmp directly; do NOT mutate the global TMPDIR — other
        // crate tests anchor `TempDir::new()` against it concurrently.
        let dir = TempDir::new_in("/tmp").unwrap();
        let saved = TREE_ENV_KEYS
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        unsafe {
            std::env::set_var("AMPLIHACK_SESSION_TREE_DIR", dir.path().join("trees"));
            std::env::remove_var("AMPLIHACK_TREE_ID");
            std::env::remove_var("AMPLIHACK_SESSION_DEPTH");
            std::env::remove_var("AMPLIHACK_MAX_DEPTH");
            std::env::remove_var("AMPLIHACK_MAX_SESSIONS");
        }
        IsolatedEnv {
            saved,
            _dir: dir,
            _lock: lock,
        }
    }

    #[test]
    fn register_root_creates_tree_and_writes_state() {
        let _env = isolated_env();
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
                    pid: None,
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
        let _env = isolated_env();
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
        let _env = isolated_env();
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
        let _env = isolated_env();
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
                    pid: None,
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

#[cfg(test)]
mod gc_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// Drive the cutoff rather than the file mtime: it needs no extra dependency
    /// and no sleeping, and it exercises exactly the same comparison.
    fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).expect("write");
        p
    }

    fn everything_is_expired() -> SystemTime {
        SystemTime::now() + Duration::from_secs(60)
    }

    fn nothing_is_expired() -> SystemTime {
        SystemTime::now() - Duration::from_secs(3600)
    }

    #[test]
    fn gc_removes_expired_tree_files_and_leaves_everything_else() {
        let td = tempfile::tempdir().expect("tempdir");
        let d = td.path();
        let json = write(d, "old.json", "aaaa");
        let lock = write(d, "old.lock", "");
        let unrelated = write(d, "notes.txt", "cc");

        let out = gc_in(d, everything_is_expired(), false).expect("gc");

        assert_eq!(out.removed, 2, "both expired tree files should be removed");
        assert_eq!(out.failed, 0);
        assert_eq!(out.bytes, 4, "only removed files' bytes are counted");
        assert!(!json.exists() && !lock.exists());
        assert!(
            unrelated.exists(),
            "non-tree files are not this pass's business"
        );
    }

    #[test]
    fn gc_leaves_files_newer_than_the_cutoff() {
        let td = tempfile::tempdir().expect("tempdir");
        let fresh = write(td.path(), "fresh.json", "bbbb");
        let out = gc_in(td.path(), nothing_is_expired(), false).expect("gc");
        assert_eq!(out, GcOutcome::default());
        assert!(
            fresh.exists(),
            "a tree inside the retention window must survive"
        );
    }

    #[test]
    fn gc_dry_run_reports_without_removing() {
        let td = tempfile::tempdir().expect("tempdir");
        let old = write(td.path(), "old.json", "aaaa");
        let out = gc_in(td.path(), everything_is_expired(), true).expect("gc");
        assert_eq!(out.removed, 1);
        assert_eq!(out.failed, 0);
        assert!(old.exists(), "--dry-run must not delete anything");
    }

    #[test]
    fn gc_on_an_empty_directory_is_a_clean_no_op() {
        let td = tempfile::tempdir().expect("tempdir");
        let out = gc_in(td.path(), everything_is_expired(), false).expect("gc");
        assert_eq!(out, GcOutcome::default());
    }

    #[test]
    fn gc_surfaces_an_unreadable_directory_instead_of_reporting_success() {
        // "removed 0 files" must never mean "I could not look".
        let td = tempfile::tempdir().expect("tempdir");
        let missing = td.path().join("does-not-exist");
        assert!(gc_in(&missing, everything_is_expired(), false).is_err());
    }
}

#[cfg(test)]
mod admit_tests {
    use super::state::{admit_session, release_session};

    /// A pid that cannot be running: above the kernel's maximum, so /proc can never
    /// contain it. Picking a large-but-plausible number would race a real process.
    const DEAD_PID: u32 = u32::MAX - 1;
    use crate::commands::session_tree::state::TreeState;

    /// Issue #1329: the cap must apply to every admission, not only to callers that
    /// go through `session-tree register`. Six concurrent `recipe run` invocations
    /// were previously admitted against a configured cap of two.
    #[test]
    fn admission_is_capped_by_max_sessions() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());

        assert!(admit_session("t", "a", 1, 3, 2).is_ok());
        assert!(admit_session("t", "b", 1, 3, 2).is_ok());
        let third = admit_session("t", "c", 1, 3, 2);
        assert!(
            third.is_err(),
            "the third admission must be refused at cap=2"
        );
        assert!(
            third.unwrap_err().to_string().contains("max_sessions"),
            "the refusal must say why"
        );
        drop(guard);
    }

    /// Capacity must come back, or a long-lived tree wedges itself.
    #[test]
    fn releasing_frees_capacity() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());

        assert!(admit_session("t", "a", 1, 3, 1).is_ok());
        assert!(admit_session("t", "b", 1, 3, 1).is_err());
        release_session("t", "a");
        assert!(
            admit_session("t", "b", 1, 3, 1).is_ok(),
            "capacity must be reusable after release"
        );
        drop(guard);
    }

    /// Depth and capacity are checked together, under one lock.
    #[test]
    fn admission_enforces_the_sealed_ceiling() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());

        assert!(admit_session("t", "root", 0, 2, 10).is_ok());
        assert!(
            admit_session("t", "ok", 2, 99, 10).is_ok(),
            "at the ceiling is allowed"
        );
        let deep = admit_session("t", "deep", 3, 99, 10);
        assert!(deep.is_err(), "past the sealed ceiling must be refused");
        assert!(deep.unwrap_err().to_string().contains("max_depth"));
        drop(guard);
    }

    /// Issue #1329: a holder killed without releasing must have its slot reclaimed.
    ///
    /// This is the branch that fixes the SIGKILL leak, and until now it was covered
    /// only by a manual check with a real `kill -9`. A pid that cannot exist stands in
    /// for a dead holder.
    #[test]
    fn a_dead_holder_does_not_hold_capacity_forever() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());

        admit_session("t", "victim", 1, 3, 1).expect("first admission");
        assert!(
            admit_session("t", "next", 1, 3, 1).is_err(),
            "cap of 1 must be full while the holder lives"
        );

        // Rewrite the holder's pid to one that cannot be running.
        let path = td.path().join("t.json");
        let mut state: TreeState =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        state
            .sessions
            .get_mut("victim")
            .expect("victim present")
            .pid = Some(DEAD_PID);
        std::fs::write(&path, serde_json::to_string(&state).expect("encode")).expect("write");

        assert!(
            admit_session("t", "next", 1, 3, 1).is_ok(),
            "a slot held by a dead process must be reclaimed, or a killed run wedges \
             the tree until the stale sweep hours later"
        );
        drop(guard);
    }

    /// The other direction, and the one that matters more: reaping a LIVE holder
    /// would over-admit -- precisely the failure the budget exists to prevent. An
    /// inverted condition here would be silent without this test.
    #[test]
    fn a_live_holder_is_never_reaped() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());

        admit_session("t", "alive", 1, 3, 1).expect("first admission");

        // The recorded pid is this test process, which is definitively running.
        let path = td.path().join("t.json");
        let state: TreeState =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(
            state.sessions["alive"].pid,
            Some(std::process::id()),
            "admission must record the holding pid, or liveness cannot be judged"
        );

        for _ in 0..3 {
            assert!(
                admit_session("t", "intruder", 1, 3, 1).is_err(),
                "a live holder must keep its slot no matter how often admission retries"
            );
        }
        drop(guard);
    }

    /// An entry written by a build that did not record pids must count as live.
    /// Treating "unknown" as dead would reclaim slots from running work.
    #[test]
    fn a_holder_without_a_recorded_pid_counts_as_live() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());

        admit_session("t", "legacy", 1, 3, 1).expect("first admission");
        let path = td.path().join("t.json");
        let mut state: TreeState =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        state.sessions.get_mut("legacy").expect("present").pid = None;
        std::fs::write(&path, serde_json::to_string(&state).expect("encode")).expect("write");

        assert!(
            admit_session("t", "next", 1, 3, 1).is_err(),
            "an entry with no recorded pid is from an older build and must be treated \
             as live; reclaiming it would over-admit"
        );
        drop(guard);
    }

    /// Admission must not silently start a second tree.
    #[test]
    fn admissions_share_one_tree_file() {
        let td = tempfile::tempdir().expect("tempdir");
        let guard = crate::test_support_tree_dir(td.path());
        admit_session("t", "a", 1, 3, 5).expect("a");
        admit_session("t", "b", 1, 3, 5).expect("b");
        let body = std::fs::read_to_string(td.path().join("t.json")).expect("tree file");
        let state: TreeState = serde_json::from_str(&body).expect("parse");
        assert_eq!(state.sessions.len(), 2);
        drop(guard);
    }
}

#[cfg(test)]
mod resource_tests {
    use super::state::{DEFAULT_MIN_AVAILABLE_MIB, available_memory_mib, memory_shortfall_mib};

    /// Reading available memory must work on the platform we actually run on, or
    /// the precondition silently becomes a no-op.
    #[test]
    #[cfg(target_os = "linux")]
    fn available_memory_is_readable() {
        let mib = available_memory_mib().expect("/proc/meminfo readable on linux");
        assert!(mib > 0, "MemAvailable should be positive, got {mib}");
    }

    /// A floor of 0 disables the check. Anyone who needs the old behaviour back
    /// must have a way to get it that does not involve editing the source.
    #[test]
    fn a_zero_floor_disables_the_check() {
        let _g = crate::test_support_env("AMPLIHACK_MIN_AVAILABLE_MIB", Some("0"));
        assert!(memory_shortfall_mib().is_none());
    }

    /// An absurd floor must refuse, proving the comparison is live rather than
    /// short-circuited.
    #[test]
    #[cfg(target_os = "linux")]
    fn an_unreachable_floor_reports_a_shortfall() {
        let _g = crate::test_support_env("AMPLIHACK_MIN_AVAILABLE_MIB", Some("999999999"));
        let (available, floor) = memory_shortfall_mib().expect("must report a shortfall");
        assert_eq!(floor, 999_999_999);
        assert!(available < floor);
    }

    /// Issue #1329, found by mutation testing: this branch never runs on a host
    /// without a cgroup limit, so it could be deleted -- or compute headroom
    /// backwards -- and every test still passed. The container case is the one it
    /// exists for, so it is exercised directly.
    #[test]
    #[cfg(target_os = "linux")]
    fn cgroup_headroom_is_max_minus_current() {
        use super::state::cgroup_headroom_mib;
        // 1 GiB limit, 256 MiB used -> 768 MiB headroom.
        assert_eq!(
            cgroup_headroom_mib("1073741824", "268435456"),
            Some(768),
            "headroom is limit minus usage, not the other way round"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn an_unlimited_cgroup_defers_to_the_host_view() {
        use super::state::cgroup_headroom_mib;
        assert_eq!(cgroup_headroom_mib("max", "12345"), None);
        assert_eq!(cgroup_headroom_mib("  max\n", "12345"), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn usage_above_the_limit_means_no_headroom_not_enormous_headroom() {
        use super::state::cgroup_headroom_mib;
        // Under reclaim pressure `current` can momentarily exceed `max`. Wrapping
        // here would report a vast amount of free memory at the worst moment.
        assert_eq!(cgroup_headroom_mib("1000", "999999999"), Some(0));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unreadable_cgroup_values_defer_rather_than_guess() {
        use super::state::cgroup_headroom_mib;
        assert_eq!(cgroup_headroom_mib("banana", "1"), None);
        assert_eq!(cgroup_headroom_mib("1000", "banana"), None);
        assert_eq!(cgroup_headroom_mib("", ""), None);
    }

    /// The default must be a real number, not accidentally zero.
    #[test]
    fn the_default_floor_is_nonzero() {
        const { assert!(DEFAULT_MIN_AVAILABLE_MIB > 0) };
    }
}
