use super::builder::EnvBuilder;
use super::helpers::*;
use std::env;
use std::path::PathBuf;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn restore_var(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => unsafe { env::set_var(name, value) },
        None => unsafe { env::remove_var(name) },
    }
}

// ── WS2: set_if ────────────────────────────────────────────────────────────

/// WS2-2a: set_if must insert the key-value pair when condition is true.
#[test]
fn set_if_sets_when_condition_true() {
    let env = EnvBuilder::new().set_if(true, "MY_KEY", "MY_VALUE").build();
    assert_eq!(
        env.get("MY_KEY").map(String::as_str),
        Some("MY_VALUE"),
        "set_if(true, ...) must insert the entry"
    );
}

/// WS2-2b: set_if must NOT insert the key-value pair when condition is false.
#[test]
fn set_if_skips_when_condition_false() {
    let env = EnvBuilder::new()
        .set_if(false, "MY_KEY", "MY_VALUE")
        .build();
    assert!(
        !env.contains_key("MY_KEY"),
        "set_if(false, ...) must not insert the entry"
    );
}

// ── Existing tests ─────────────────────────────────────────────────────────

#[test]
fn empty_builder_produces_empty_map() {
    let env = EnvBuilder::new().build();
    assert!(env.is_empty());
}

#[test]
fn set_adds_variable() {
    let env = EnvBuilder::new()
        .set("FOO", "bar")
        .set("BAZ", "qux")
        .build();
    assert_eq!(env.get("FOO").unwrap(), "bar");
    assert_eq!(env.get("BAZ").unwrap(), "qux");
}

#[test]
fn prepend_path_deduplicates() {
    let env = EnvBuilder::new()
        .prepend_path("/opt/bin")
        .prepend_path("/opt/bin") // duplicate
        .build();

    let path = env.get("PATH").unwrap();
    let count = path.matches("/opt/bin").count();
    assert_eq!(count, 1, "PATH should not contain duplicates");
}

#[test]
fn with_amplihack_session_id_sets_vars() {
    let env = EnvBuilder::new().with_amplihack_session_id().build();
    assert!(env.contains_key("AMPLIHACK_SESSION_ID"));
    assert!(env.contains_key("AMPLIHACK_DEPTH"));
}

#[test]
fn with_session_tree_context_preserves_existing_values() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev_tree = env::var_os("AMPLIHACK_TREE_ID");
    let prev_depth = env::var_os("AMPLIHACK_SESSION_DEPTH");
    let prev_max_depth = env::var_os("AMPLIHACK_MAX_DEPTH");
    let prev_max_sessions = env::var_os("AMPLIHACK_MAX_SESSIONS");
    unsafe {
        env::set_var("AMPLIHACK_TREE_ID", "tree1234");
        env::set_var("AMPLIHACK_SESSION_DEPTH", "2");
        env::set_var("AMPLIHACK_MAX_DEPTH", "4");
        env::set_var("AMPLIHACK_MAX_SESSIONS", "12");
    }

    let built = EnvBuilder::new().with_session_tree_context().build();

    restore_var("AMPLIHACK_TREE_ID", prev_tree);
    restore_var("AMPLIHACK_SESSION_DEPTH", prev_depth);
    restore_var("AMPLIHACK_MAX_DEPTH", prev_max_depth);
    restore_var("AMPLIHACK_MAX_SESSIONS", prev_max_sessions);

    assert_eq!(
        built.get("AMPLIHACK_TREE_ID").map(String::as_str),
        Some("tree1234")
    );
    assert_eq!(
        built.get("AMPLIHACK_SESSION_DEPTH").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        built.get("AMPLIHACK_MAX_DEPTH").map(String::as_str),
        Some("4")
    );
    assert_eq!(
        built.get("AMPLIHACK_MAX_SESSIONS").map(String::as_str),
        Some("12")
    );
}

#[test]
fn with_incremented_session_tree_context_increments_depth() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev_tree = env::var_os("AMPLIHACK_TREE_ID");
    let prev_depth = env::var_os("AMPLIHACK_SESSION_DEPTH");
    let prev_max_depth = env::var_os("AMPLIHACK_MAX_DEPTH");
    let prev_max_sessions = env::var_os("AMPLIHACK_MAX_SESSIONS");
    unsafe {
        env::set_var("AMPLIHACK_TREE_ID", "tree1234");
        env::set_var("AMPLIHACK_SESSION_DEPTH", "2");
        env::set_var("AMPLIHACK_MAX_DEPTH", "4");
        env::set_var("AMPLIHACK_MAX_SESSIONS", "12");
    }

    let built = EnvBuilder::new()
        .with_incremented_session_tree_context()
        .build();

    restore_var("AMPLIHACK_TREE_ID", prev_tree);
    restore_var("AMPLIHACK_SESSION_DEPTH", prev_depth);
    restore_var("AMPLIHACK_MAX_DEPTH", prev_max_depth);
    restore_var("AMPLIHACK_MAX_SESSIONS", prev_max_sessions);

    assert_eq!(
        built.get("AMPLIHACK_SESSION_DEPTH").map(String::as_str),
        Some("3")
    );
    assert_eq!(
        built.get("AMPLIHACK_TREE_ID").map(String::as_str),
        Some("tree1234")
    );
}

#[test]
fn with_amplihack_vars_sets_runtime_flag() {
    let env = EnvBuilder::new().with_amplihack_vars().build();
    assert_eq!(env.get("AMPLIHACK_RUST_RUNTIME").unwrap(), "1");
    assert!(env.contains_key("AMPLIHACK_VERSION"));
}

#[test]
fn with_amplihack_vars_with_node_options_uses_explicit_value() {
    let env = EnvBuilder::new()
        .with_amplihack_vars_with_node_options(Some("--max-old-space-size=16384 --inspect"))
        .build();
    assert_eq!(
        env.get("NODE_OPTIONS").map(String::as_str),
        Some("--max-old-space-size=16384 --inspect")
    );
}

#[test]
fn with_asset_resolver_sets_from_path() {
    let temp = tempfile::tempdir().unwrap();
    let resolver = temp.path().join("amplihack-asset-resolver");
    std::fs::write(&resolver, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&resolver, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let prev_path = env::var_os("PATH");
    let prev_resolver = env::var_os("AMPLIHACK_ASSET_RESOLVER");
    unsafe {
        env::set_var("PATH", temp.path());
        env::remove_var("AMPLIHACK_ASSET_RESOLVER");
    }

    let built = EnvBuilder::new().with_asset_resolver().build();

    match prev_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match prev_resolver {
        Some(value) => unsafe { env::set_var("AMPLIHACK_ASSET_RESOLVER", value) },
        None => unsafe { env::remove_var("AMPLIHACK_ASSET_RESOLVER") },
    }

    assert_eq!(
        built.get("AMPLIHACK_ASSET_RESOLVER").map(String::as_str),
        Some(resolver.to_str().unwrap())
    );
}

#[test]
fn generate_session_id_format() {
    let id = generate_session_id();
    assert!(
        id.starts_with("rs-"),
        "session ID should start with 'rs-': {id}"
    );
}

#[test]
fn build_path_preserves_order() {
    let path = build_path(
        &[PathBuf::from("/first"), PathBuf::from("/second")],
        "/third:/fourth",
    );
    let parts: Vec<&str> = path.split(':').collect();
    assert_eq!(parts[0], "/first");
    assert_eq!(parts[1], "/second");
    assert_eq!(parts[2], "/third");
    assert_eq!(parts[3], "/fourth");
}
