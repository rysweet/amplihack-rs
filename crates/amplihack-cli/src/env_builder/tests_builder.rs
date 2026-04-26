use super::builder::EnvBuilder;
use super::helpers::*;
use crate::test_support::cwd_env_lock;
use std::collections::HashMap;
use std::env;
use std::process::Command;

// ── WS1: with_agent_binary ────────────────────────────────────────────────

/// WS1-1: with_agent_binary must insert AMPLIHACK_AGENT_BINARY for each
/// supported tool name.
#[test]
fn with_agent_binary_sets_env_var_for_all_tools() {
    for tool in &["claude", "copilot", "codex", "amplifier"] {
        let env = EnvBuilder::new().with_agent_binary(*tool).build();
        assert_eq!(
            env.get("AMPLIHACK_AGENT_BINARY").map(String::as_str),
            Some(*tool),
            "AMPLIHACK_AGENT_BINARY should be '{tool}'"
        );
    }
}

#[test]
fn active_agent_binary_reads_env_override() {
    let previous = env::var_os("AMPLIHACK_AGENT_BINARY");
    unsafe { env::set_var("AMPLIHACK_AGENT_BINARY", "copilot") };

    let binary = active_agent_binary();

    match previous {
        Some(value) => unsafe { env::set_var("AMPLIHACK_AGENT_BINARY", value) },
        None => unsafe { env::remove_var("AMPLIHACK_AGENT_BINARY") },
    }

    assert_eq!(binary, "copilot");
}

#[test]
fn with_project_graph_db_sets_project_local_path() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let env = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap()
        .build();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let expected = temp.path().join(".amplihack").join("graph_db");
    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some(expected.to_str().unwrap())
    );
    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
}

/// Issue #250: explicit `project_root` is authoritative — an inherited
/// `AMPLIHACK_KUZU_DB_PATH` legacy alias must NOT override the derived path.
#[test]
fn with_project_graph_db_ignores_inherited_legacy_kuzu_alias() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/legacy-graph-alias") };

    let env = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap()
        .build();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let expected = temp.path().join(".amplihack").join("graph_db");
    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some(expected.to_str().unwrap()),
        "explicit project_root must win over inherited AMPLIHACK_KUZU_DB_PATH"
    );
    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
}

/// Issue #250: explicit `project_root` is authoritative — an inherited
/// `AMPLIHACK_GRAPH_DB_PATH` must NOT override the derived path. Legacy
/// alias is still removed so only the neutral contract propagates.
#[test]
fn with_project_graph_db_ignores_inherited_graph_db_env() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/custom/graph-only") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let env = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap()
        .build();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let expected = temp.path().join(".amplihack").join("graph_db");
    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some(expected.to_str().unwrap()),
        "explicit project_root must win over inherited AMPLIHACK_GRAPH_DB_PATH"
    );
    assert_eq!(
        env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str),
        None,
        "AMPLIHACK_KUZU_DB_PATH must not be re-emitted into child overrides"
    );
}

/// Issue #250: when both legacy and neutral envs are set in the parent process,
/// the explicit `project_root` still wins; the legacy alias is unset.
#[test]
fn with_project_graph_db_ignores_both_inherited_envs() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/custom/graph") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/legacy-graph-alias") };

    let env = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap()
        .build();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let expected = temp.path().join(".amplihack").join("graph_db");
    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some(expected.to_str().unwrap()),
        "explicit project_root must win over both inherited envs"
    );
    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
}

/// Issue #250: bogus inherited env values (relative, /proc-prefixed, traversal)
/// are ignored entirely — `with_project_graph_db` never reads them, so they
/// can never cause errors or leak into child processes.
#[test]
fn with_project_graph_db_ignores_bogus_inherited_envs() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");

    for bogus in &["relative/graph_db", "/proc/1/mem", "/tmp/../etc/shadow"] {
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", bogus) };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", bogus) };

        let env = EnvBuilder::new()
            .with_project_graph_db(temp.path())
            .expect("bogus inherited env must NOT cause an error")
            .build();

        let expected = temp.path().join(".amplihack").join("graph_db");
        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some(expected.to_str().unwrap()),
            "bogus inherited env {bogus:?} must not leak into child override"
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }
}

#[test]
fn apply_to_command_translates_kuzu_alias_to_graph_db_path_and_removes_kuzu_var() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/inherited/legacy") };

    let mut cmd = Command::new("true");
    EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap()
        .apply_to_command(&mut cmd);

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let envs: HashMap<_, _> = cmd
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    let expected = temp.path().join(".amplihack").join("graph_db");
    assert_eq!(
        envs.get("AMPLIHACK_GRAPH_DB_PATH")
            .and_then(|value| value.as_deref()),
        Some(expected.to_str().unwrap()),
        "explicit project_root must win; inherited KUZU_DB_PATH is ignored (issue #250)"
    );
    assert_eq!(
        envs.get("AMPLIHACK_KUZU_DB_PATH")
            .and_then(|value| value.as_deref()),
        None,
        "Command must explicitly remove inherited AMPLIHACK_KUZU_DB_PATH"
    );
}

// ── WS3: with_amplihack_home ───────────────────────────────────────────────

/// WS3-1: with_amplihack_home should derive AMPLIHACK_HOME from HOME when
/// AMPLIHACK_HOME is not set.
#[test]
fn with_amplihack_home_sets_from_home() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let temp = tempfile::tempdir().unwrap();
    let prev_home = crate::test_support::set_home(temp.path());
    let prev_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe { std::env::remove_var("AMPLIHACK_HOME") };

    let env = EnvBuilder::new().with_amplihack_home().build();

    crate::test_support::restore_home(prev_home);
    match prev_amplihack_home {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }

    let expected = temp.path().join(".amplihack");
    assert_eq!(
        env.get("AMPLIHACK_HOME").map(String::as_str),
        Some(expected.to_str().unwrap()),
        "AMPLIHACK_HOME should be <HOME>/.amplihack when unset"
    );
}

/// WS3-2: with_amplihack_home must not overwrite an AMPLIHACK_HOME that is
/// already set in the process environment.
#[test]
fn with_amplihack_home_does_not_overwrite_existing() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let custom = "/custom/path";
    let prev = std::env::var_os("AMPLIHACK_HOME");
    unsafe { std::env::set_var("AMPLIHACK_HOME", custom) };

    let env = EnvBuilder::new().with_amplihack_home().build();

    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }

    assert_eq!(
        env.get("AMPLIHACK_HOME").map(String::as_str),
        Some(custom),
        "with_amplihack_home must preserve a pre-existing AMPLIHACK_HOME"
    );
}

/// WS3-3 (SEC-WS3-01): with_amplihack_home must reject a HOME that contains
/// path traversal components (e.g. "..") and must NOT set AMPLIHACK_HOME.
#[test]
fn with_amplihack_home_rejects_traversal_path() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let prev_home = crate::test_support::set_home(std::path::Path::new("/tmp/../../etc"));
    let prev_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe { std::env::remove_var("AMPLIHACK_HOME") };

    let env = EnvBuilder::new().with_amplihack_home().build();

    crate::test_support::restore_home(prev_home);
    match prev_amplihack_home {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }

    assert!(
        !env.contains_key("AMPLIHACK_HOME"),
        "with_amplihack_home must not set AMPLIHACK_HOME when HOME contains path traversal"
    );
}

// ── Issue #441: active_agent_binary parent-runtime detection ──────────────
//
// These tests serialize all mutations to the detection-relevant env vars
// via a single mutex, snapshot every relevant key on entry, scrub them, run
// the assertion, then restore originals on drop (RAII).

use std::sync::Mutex;

static AGENT_BINARY_ENV_LOCK: Mutex<()> = Mutex::new(());

const TRACKED_EXACT: &[&str] = &[
    "AMPLIHACK_AGENT_BINARY",
    "COPILOT_AGENT_SESSION_ID",
    "CLAUDECODE",
    "CODEX_HOME",
];

const TRACKED_PREFIXES: &[&str] = &["CLAUDE_CODE_", "CODEX_"];

struct EnvSnapshot {
    saved: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl EnvSnapshot {
    /// Snapshot every detection-relevant env var, then remove them all.
    fn scrub_all() -> Self {
        let mut saved = Vec::new();
        let mut keys: Vec<std::ffi::OsString> = Vec::new();
        for k in TRACKED_EXACT {
            keys.push(std::ffi::OsString::from(k));
        }
        for (k, _v) in env::vars_os() {
            let k_lossy = k.to_string_lossy();
            for prefix in TRACKED_PREFIXES {
                if k_lossy.starts_with(prefix) && !keys.iter().any(|existing| existing == &k) {
                    keys.push(k.clone());
                    break;
                }
            }
        }
        for k in keys {
            if let Some(v) = env::var_os(&k) {
                saved.push((k.clone(), v));
            }
            unsafe { env::remove_var(&k) };
        }
        Self { saved }
    }
}

impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        // Remove anything we may have set during the test, then restore.
        for k in TRACKED_EXACT {
            unsafe { env::remove_var(k) };
        }
        // We cannot enumerate test-set prefixed keys reliably; clear any
        // tracked keys that were saved + any that exist now matching prefix.
        let current: Vec<std::ffi::OsString> = env::vars_os().map(|(k, _)| k).collect();
        for k in current {
            let k_lossy = k.to_string_lossy();
            for prefix in TRACKED_PREFIXES {
                if k_lossy.starts_with(prefix) {
                    unsafe { env::remove_var(&k) };
                    break;
                }
            }
        }
        for (k, v) in &self.saved {
            unsafe { env::set_var(k, v) };
        }
    }
}

fn lock_agent_env() -> std::sync::MutexGuard<'static, ()> {
    AGENT_BINARY_ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Issue #441: explicit AMPLIHACK_AGENT_BINARY override beats any runtime
/// signal — including when copilot/claude/codex env vars are also present.
#[test]
fn override_wins_over_runtime_signals() {
    let _guard = lock_agent_env();
    let _snap = EnvSnapshot::scrub_all();

    unsafe { env::set_var("AMPLIHACK_AGENT_BINARY", "custom") };
    unsafe { env::set_var("COPILOT_AGENT_SESSION_ID", "abc123") };
    unsafe { env::set_var("CLAUDECODE", "1") };
    unsafe { env::set_var("CODEX_HOME", "/x") };

    assert_eq!(active_agent_binary(), "custom");
}

/// Issue #441: COPILOT_AGENT_SESSION_ID alone -> "copilot".
#[test]
fn detects_copilot_via_session_id() {
    let _guard = lock_agent_env();
    let _snap = EnvSnapshot::scrub_all();

    unsafe { env::set_var("COPILOT_AGENT_SESSION_ID", "session-xyz") };

    assert_eq!(active_agent_binary(), "copilot");
}

/// Issue #441: CLAUDECODE set OR any CLAUDE_CODE_* prefix set -> "claude".
#[test]
fn detects_claude_via_claudecode_or_prefix() {
    // Sub-case 1: CLAUDECODE exact match
    {
        let _guard = lock_agent_env();
        let _snap = EnvSnapshot::scrub_all();
        unsafe { env::set_var("CLAUDECODE", "1") };
        assert_eq!(
            active_agent_binary(),
            "claude",
            "CLAUDECODE should select claude"
        );
    }
    // Sub-case 2: CLAUDE_CODE_* prefix match
    {
        let _guard = lock_agent_env();
        let _snap = EnvSnapshot::scrub_all();
        unsafe { env::set_var("CLAUDE_CODE_SESSION", "x") };
        assert_eq!(
            active_agent_binary(),
            "claude",
            "CLAUDE_CODE_* prefix should select claude"
        );
    }
}

/// Issue #441: CODEX_HOME set OR any CODEX_* prefix set -> "codex".
#[test]
fn detects_codex_via_home_or_prefix() {
    // Sub-case 1: CODEX_HOME exact match
    {
        let _guard = lock_agent_env();
        let _snap = EnvSnapshot::scrub_all();
        unsafe { env::set_var("CODEX_HOME", "/opt/codex") };
        assert_eq!(
            active_agent_binary(),
            "codex",
            "CODEX_HOME should select codex"
        );
    }
    // Sub-case 2: CODEX_* prefix match (not CODEX_HOME)
    {
        let _guard = lock_agent_env();
        let _snap = EnvSnapshot::scrub_all();
        unsafe { env::set_var("CODEX_SESSION_ID", "x") };
        assert_eq!(
            active_agent_binary(),
            "codex",
            "CODEX_* prefix should select codex"
        );
    }
}

/// Issue #441: when no detection vars are set, fall back to "claude" with warn.
#[test]
fn fallback_when_nothing_set() {
    let _guard = lock_agent_env();
    let _snap = EnvSnapshot::scrub_all();

    assert_eq!(active_agent_binary(), "claude");
}

/// Issue #441: empty/whitespace-only override is treated as "not set" and
/// detection logic proceeds (here: copilot via session id).
#[test]
fn empty_override_falls_through_to_detection() {
    let _guard = lock_agent_env();
    let _snap = EnvSnapshot::scrub_all();

    unsafe { env::set_var("AMPLIHACK_AGENT_BINARY", "   ") };
    unsafe { env::set_var("COPILOT_AGENT_SESSION_ID", "session") };

    assert_eq!(active_agent_binary(), "copilot");
}
