use super::builder::EnvBuilder;
use super::helpers::*;
use crate::test_support::cwd_env_lock;
use std::collections::HashMap;
use std::env;
use std::process::Command;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn restore_var(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => unsafe { env::set_var(name, value) },
        None => unsafe { env::remove_var(name) },
    }
}

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

#[test]
fn with_project_graph_db_preserves_existing_override() {
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

    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some("/custom/legacy-graph-alias")
    );
    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
}

/// I77-ENV-GRAPH-ONLY: When only AMPLIHACK_GRAPH_DB_PATH is set,
/// with_project_graph_db() must preserve the backend-neutral name and avoid
/// re-emitting the legacy Kuzu alias into child process overrides.
#[test]
fn with_project_graph_db_preserves_graph_db_env_without_emitting_legacy_alias() {
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

    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some("/custom/graph-only"),
        "AMPLIHACK_GRAPH_DB_PATH must be preserved from the process environment"
    );
    assert_eq!(
        env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str),
        None,
        "AMPLIHACK_KUZU_DB_PATH must not be re-emitted into child overrides"
    );
}

#[test]
fn with_project_graph_db_prefers_backend_neutral_override() {
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

    assert_eq!(
        env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
        Some("/custom/graph")
    );
    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
}

#[test]
fn with_project_graph_db_rejects_relative_graph_override() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "relative/graph_db") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("graph DB path must be absolute"));
}

#[test]
fn with_project_graph_db_rejects_proc_prefixed_graph_override() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/proc/1/mem") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("blocked prefix /proc"));
}

#[test]
fn with_project_graph_db_invalid_graph_override_does_not_fall_through_to_kuzu_alias() {
    let _guard = cwd_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/../etc/shadow") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/legacy-graph-alias") };

    let error = EnvBuilder::new()
        .with_project_graph_db(temp.path())
        .unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("/tmp/../etc/shadow"));
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
    assert_eq!(
        envs.get("AMPLIHACK_GRAPH_DB_PATH")
            .and_then(|value| value.as_deref()),
        Some("/inherited/legacy"),
        "Legacy KUZU_DB_PATH must be translated to GRAPH_DB_PATH in child env"
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
