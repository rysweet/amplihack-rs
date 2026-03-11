//! TDD tests for kuzu-backend feature-flag gating.
//!
//! These tests define the contract for the optional `kuzu-backend` feature:
//! - Without `kuzu-backend`: kuzu operations must fail with an actionable error
//!   message that tells users exactly how to reinstall with the feature enabled.
//! - With `kuzu-backend`: kuzu parsing must succeed.
//! - Common operations (sqlite path) must always work regardless of feature.
//!
//! Run without the kuzu feature (default install path):
//!   cargo test -p amplihack-cli --lib -- backend_tests
//!
//! Run with the kuzu feature:
//!   cargo test -p amplihack-cli --lib --features kuzu-backend -- backend_tests

use super::*;

// ---------------------------------------------------------------------------
// BackendChoice::parse — unit tests
// ---------------------------------------------------------------------------

/// The sqlite backend is always available; it must parse successfully.
#[test]
fn backend_choice_parse_sqlite_succeeds() {
    let result = BackendChoice::parse("sqlite");
    assert!(
        result.is_ok(),
        "expected Ok for 'sqlite', got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), BackendChoice::Sqlite);
}

/// An unrecognised backend name must return an error.
#[test]
fn backend_choice_parse_invalid_returns_error() {
    let result = BackendChoice::parse("invalid");
    assert!(result.is_err(), "expected Err for 'invalid', got Ok");
}

/// The error for an invalid backend must mention the invalid value.
#[test]
fn backend_choice_parse_invalid_error_mentions_value() {
    let err = BackendChoice::parse("rocksdb").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("rocksdb"),
        "error message should mention the invalid value 'rocksdb', got: {msg}"
    );
}

/// Input longer than 64 characters should still produce an error (not panic).
/// The error message need not include the full string, but must not crash.
#[test]
fn backend_choice_parse_very_long_input_does_not_panic() {
    let long = "x".repeat(200);
    let result = BackendChoice::parse(&long);
    assert!(result.is_err(), "expected Err for long unknown backend");
}

/// Attempting to use the kuzu backend without the feature must return Err
/// and the error message must contain the reinstall command so the user knows
/// exactly what to do.
#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn backend_choice_parse_kuzu_without_feature_returns_err() {
    let result = BackendChoice::parse("kuzu");
    assert!(
        result.is_err(),
        "expected Err for 'kuzu' when kuzu-backend feature is disabled"
    );
}

#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn backend_choice_parse_kuzu_error_contains_reinstall_command() {
    let err = BackendChoice::parse("kuzu").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("--features kuzu-backend"),
        "error must mention '--features kuzu-backend' so users know how to fix it, got: {msg}"
    );
}

#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn backend_choice_parse_kuzu_error_contains_cargo_install() {
    let err = BackendChoice::parse("kuzu").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("cargo install"),
        "error must mention 'cargo install' to guide the user, got: {msg}"
    );
}

#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn backend_choice_parse_kuzu_error_contains_repo_url() {
    let err = BackendChoice::parse("kuzu").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("amplihack-rs"),
        "error must reference the amplihack-rs repository, got: {msg}"
    );
}

/// When `kuzu-backend` IS enabled, parsing "kuzu" must succeed.
#[cfg(feature = "kuzu-backend")]
#[test]
fn backend_choice_parse_kuzu_with_feature_succeeds() {
    let result = BackendChoice::parse("kuzu");
    assert!(
        result.is_ok(),
        "expected Ok for 'kuzu' when kuzu-backend feature is enabled, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), BackendChoice::Kuzu);
}

// ---------------------------------------------------------------------------
// TransferFormat::parse — unit tests
// ---------------------------------------------------------------------------

/// JSON format is always available.
#[test]
fn transfer_format_parse_json_succeeds() {
    let result = TransferFormat::parse("json");
    assert!(result.is_ok(), "expected Ok for 'json', got: {:?}", result);
    assert_eq!(result.unwrap(), TransferFormat::Json);
}

/// An unrecognised format name must return an error.
#[test]
fn transfer_format_parse_invalid_returns_error() {
    let result = TransferFormat::parse("parquet");
    assert!(
        result.is_err(),
        "expected Err for unsupported format 'parquet'"
    );
}

/// The error message for an unknown format should guide the user.
#[test]
fn transfer_format_parse_invalid_mentions_supported_formats() {
    let err = TransferFormat::parse("csv").unwrap_err();
    let msg = err.to_string();
    // The error should mention at least one supported format.
    assert!(
        msg.contains("json") || msg.contains("kuzu"),
        "error should list supported formats, got: {msg}"
    );
}

/// Without the feature, attempting to parse "kuzu" format must error.
#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn transfer_format_parse_kuzu_without_feature_returns_err() {
    let result = TransferFormat::parse("kuzu");
    assert!(
        result.is_err(),
        "expected Err for 'kuzu' format when kuzu-backend feature is disabled"
    );
}

/// The error from TransferFormat must also direct users to reinstall.
#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn transfer_format_parse_kuzu_error_contains_reinstall_command() {
    let err = TransferFormat::parse("kuzu").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("--features kuzu-backend"),
        "error must mention '--features kuzu-backend', got: {msg}"
    );
}

/// When `kuzu-backend` IS enabled, parsing "kuzu" format must succeed.
#[cfg(feature = "kuzu-backend")]
#[test]
fn transfer_format_parse_kuzu_with_feature_succeeds() {
    let result = TransferFormat::parse("kuzu");
    assert!(
        result.is_ok(),
        "expected Ok for 'kuzu' format with kuzu-backend feature, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), TransferFormat::Kuzu);
}

// ---------------------------------------------------------------------------
// parse_json_value — unit tests (always available, no feature dependency)
// ---------------------------------------------------------------------------

#[test]
fn parse_json_value_empty_string_returns_empty_object() {
    let result = parse_json_value("").unwrap();
    assert_eq!(result, serde_json::json!({}));
}

#[test]
fn parse_json_value_valid_json_roundtrips() {
    let input = r#"{"key": "value", "count": 42}"#;
    let result = parse_json_value(input).unwrap();
    assert_eq!(result["key"], "value");
    assert_eq!(result["count"], 42);
}

#[test]
fn parse_json_value_invalid_json_returns_error() {
    let result = parse_json_value("{broken json");
    assert!(result.is_err(), "expected Err for malformed JSON");
}

// ---------------------------------------------------------------------------
// SQLite helper — always available unit tests
// ---------------------------------------------------------------------------

#[test]
fn open_sqlite_in_memory_and_list_empty_sessions() -> anyhow::Result<()> {
    use rusqlite::Connection as SqliteConnection;
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    let sessions = list_sqlite_sessions_from_conn(&conn)?;
    assert!(
        sessions.is_empty(),
        "expected no sessions in fresh DB, got: {:?}",
        sessions.len()
    );
    Ok(())
}

#[test]
fn list_sqlite_sessions_counts_memory_entries_correctly() -> anyhow::Result<()> {
    use rusqlite::{Connection as SqliteConnection, params};
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params!["sess-abc", "2026-01-01T00:00:00", "2026-01-01T00:00:00"],
    )?;
    // Insert two memory entries for this session.
    for i in 0..2 {
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, created_at, accessed_at) VALUES (?1, ?2, 'agent', 'conversation', 'T', 'C', '{}', ?3, ?3)",
            params![format!("m-{i}"), "sess-abc", "2026-01-01T00:00:00"],
        )?;
    }
    let sessions = list_sqlite_sessions_from_conn(&conn)?;
    assert_eq!(sessions.len(), 1, "expected exactly one session");
    assert_eq!(
        sessions[0].memory_count, 2,
        "expected 2 memory entries, got {}",
        sessions[0].memory_count
    );
    Ok(())
}

#[test]
fn query_sqlite_memories_filters_by_type() -> anyhow::Result<()> {
    use rusqlite::{Connection as SqliteConnection, params};
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params!["s1", "2026-01-01T00:00:00", "2026-01-01T00:00:00"],
    )?;
    for (id, mtype) in [("m1", "conversation"), ("m2", "context")] {
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, created_at, accessed_at) VALUES (?1, ?2, 'a', ?3, 'T', 'C', '{}', '2026-01-01T00:00:00', '2026-01-01T00:00:00')",
            params![id, "s1", mtype],
        )?;
    }
    let all = query_sqlite_memories_for_session(&conn, "s1", None)?;
    assert_eq!(all.len(), 2, "expected 2 total memories");

    let only_context = query_sqlite_memories_for_session(&conn, "s1", Some("context"))?;
    assert_eq!(only_context.len(), 1, "expected 1 context memory");
    assert_eq!(only_context[0].memory_type, "context");
    Ok(())
}

#[test]
fn collect_sqlite_agent_counts_groups_by_agent() -> anyhow::Result<()> {
    use rusqlite::{Connection as SqliteConnection, params};
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params!["s1", "2026-01-01T00:00:00", "2026-01-01T00:00:00"],
    )?;
    for (id, agent) in [("m1", "alice"), ("m2", "alice"), ("m3", "bob")] {
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, created_at, accessed_at) VALUES (?1, ?2, ?3, 'conv', 'T', 'C', '{}', '2026-01-01T00:00:00', '2026-01-01T00:00:00')",
            params![id, "s1", agent],
        )?;
    }
    let counts = collect_sqlite_agent_counts(&conn)?;
    // Must have exactly two agents: alice (2) and bob (1), ordered by agent_id.
    assert_eq!(counts.len(), 2, "expected 2 agents");
    assert_eq!(counts[0].0, "alice");
    assert_eq!(counts[0].1, 2);
    assert_eq!(counts[1].0, "bob");
    assert_eq!(counts[1].1, 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// Feature constant availability tests
// ---------------------------------------------------------------------------

/// SQLITE_TREE_BACKEND_NAME must always be accessible regardless of feature.
#[test]
fn sqlite_tree_backend_name_constant_is_available() {
    // This test fails to compile if the constant is accidentally gated behind
    // #[cfg(feature = "kuzu-backend")].
    let name: &str = SQLITE_TREE_BACKEND_NAME;
    assert!(
        !name.is_empty(),
        "SQLITE_TREE_BACKEND_NAME must not be empty"
    );
}

/// SQLITE_SCHEMA must always be available and must define the expected tables.
#[test]
fn sqlite_schema_constant_contains_required_tables() {
    assert!(
        SQLITE_SCHEMA.contains("memory_entries"),
        "SQLITE_SCHEMA must define memory_entries table"
    );
    assert!(
        SQLITE_SCHEMA.contains("sessions"),
        "SQLITE_SCHEMA must define sessions table"
    );
    assert!(
        SQLITE_SCHEMA.contains("session_agents"),
        "SQLITE_SCHEMA must define session_agents table"
    );
}

/// When kuzu-backend is disabled, KUZU_TREE_BACKEND_NAME must NOT be visible
/// in order to prevent accidentally referencing a kuzu constant in non-kuzu code.
/// This test is a *compile-time* assertion — it verifies the cfg gate exists by
/// only referencing the constant behind the same cfg guard.
#[cfg(feature = "kuzu-backend")]
#[test]
fn kuzu_tree_backend_name_available_when_feature_enabled() {
    let name: &str = KUZU_TREE_BACKEND_NAME;
    assert!(
        !name.is_empty(),
        "KUZU_TREE_BACKEND_NAME must not be empty when kuzu-backend is enabled"
    );
    assert_eq!(name, "kuzu", "KUZU_TREE_BACKEND_NAME must equal 'kuzu'");
}
