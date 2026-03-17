//! Kuzu C++ FFI integration tests.
//!
//! These tests exercise the actual kuzu C++ FFI through the cxx bridge.
//! They will FAIL AT LINK TIME if cxx-build has a different minor version
//! than cxx, producing errors like:
//!   undefined reference to `cxxbridge1$string$new$1_0_138'
//!
//! After the fix (cargo update -p cxx-build --precise 1.0.138), the
//! linker resolves all symbols and these tests compile and run.
//!
//! Closes: https://github.com/rysweet/amplihack-rs/issues/35
//!
//! These tests live in `tests/integration/` (not in the production
//! `memory/mod.rs` module) so that kuzu FFI smoke tests are isolated
//! from production code and follow the standard Rust integration test
//! layout (one test per concern, outside the library under test).

use amplihack_cli::memory::ffi_test_support::{
    graph_rows, init_graph_backend_schema, list_graph_sessions_from_conn,
};
use anyhow::Result;
use kuzu::{
    Connection as KuzuConn, Database as KuzuDb, SystemConfig as KuzuSysCfg, Value as KuzuValue,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Create a temporary kuzu database in an isolated directory.
///
/// Returns the TempDir (kept alive for the test scope) and the Database.
fn temp_kuzu_db() -> Result<(TempDir, KuzuDb)> {
    let dir = TempDir::new().map_err(|e| anyhow::anyhow!("tempdir: {e}"))?;
    let db = KuzuDb::new(dir.path().join("test.kuzu"), KuzuSysCfg::default())
        .map_err(|e| anyhow::anyhow!("kuzu open: {e}"))?;
    Ok((dir, db))
}

// ---------------------------------------------------------------------------
// FFI smoke tests — verify the cxx bridge is link-compatible
// ---------------------------------------------------------------------------

/// Verify that the kuzu Database can be created.
///
/// This is the simplest possible kuzu FFI smoke test.  If this test
/// fails to compile, the cxx/cxx-build version mismatch is present.
#[test]
fn kuzu_ffi_database_opens() -> Result<()> {
    let (_dir, _db) = temp_kuzu_db()?;
    Ok(())
}

/// Verify that a Connection can be created from a Database.
///
/// Connection creation crosses the cxx bridge: it calls the C++ constructor
/// via a generated bridge symbol.
#[test]
fn kuzu_ffi_connection_opens() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let _conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("connection: {e}"))?;
    Ok(())
}

/// Verify that a trivial query executes through the C++ query engine.
///
/// `RETURN 1` exercises the full Rust→C++ FFI path: query string is passed
/// over the bridge, the C++ engine evaluates it, and a QueryResult is
/// returned over the bridge back to Rust.
#[test]
fn kuzu_ffi_basic_query_executes() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = conn
        .query("RETURN 1")
        .map_err(|e| anyhow::anyhow!("RETURN 1 failed: {e}"))?;
    let rows: Vec<Vec<KuzuValue>> = result.collect();
    assert_eq!(rows.len(), 1, "RETURN 1 must yield exactly one row");
    Ok(())
}

/// Verify that a node table can be defined via DDL.
///
/// CREATE NODE TABLE involves schema mutation across the cxx bridge,
/// exercising bridge symbols for string-passing and error propagation.
#[test]
fn kuzu_ffi_node_table_ddl() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
    conn.query("CREATE NODE TABLE IF NOT EXISTS Ping(id STRING, PRIMARY KEY (id))")
        .map_err(|e| anyhow::anyhow!("CREATE NODE TABLE failed: {e}"))?;
    Ok(())
}

/// Verify that a relationship table can be defined via DDL.
///
/// Relationship tables add an additional level of schema complexity
/// and exercise the C++ catalog more deeply than node tables.
#[test]
fn kuzu_ffi_rel_table_ddl() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
    conn.query("CREATE NODE TABLE IF NOT EXISTS SrcNode(id STRING, PRIMARY KEY (id))")
        .map_err(|e| anyhow::anyhow!("CREATE SrcNode: {e}"))?;
    conn.query("CREATE NODE TABLE IF NOT EXISTS DstNode(id STRING, PRIMARY KEY (id))")
        .map_err(|e| anyhow::anyhow!("CREATE DstNode: {e}"))?;
    conn.query("CREATE REL TABLE IF NOT EXISTS LINKED(FROM SrcNode TO DstNode, weight DOUBLE)")
        .map_err(|e| anyhow::anyhow!("CREATE REL TABLE: {e}"))?;
    Ok(())
}

/// Verify the full insert → query round-trip through the cxx bridge.
///
/// This exercises Rust→C++ value marshaling in both directions:
///   - INSERT: Rust strings are passed to C++ storage
///   - MATCH:  C++ values are returned to Rust as kuzu::Value
#[test]
fn kuzu_ffi_insert_and_query_round_trip() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;

    conn.query(
        "CREATE NODE TABLE IF NOT EXISTS Msg(msg_id STRING, body STRING, PRIMARY KEY (msg_id))",
    )
    .map_err(|e| anyhow::anyhow!("schema: {e}"))?;
    conn.query("CREATE (:Msg {msg_id: 'm1', body: 'hello'})")
        .map_err(|e| anyhow::anyhow!("insert m1: {e}"))?;
    conn.query("CREATE (:Msg {msg_id: 'm2', body: 'world'})")
        .map_err(|e| anyhow::anyhow!("insert m2: {e}"))?;

    let result = conn
        .query("MATCH (m:Msg) RETURN m.msg_id ORDER BY m.msg_id")
        .map_err(|e| anyhow::anyhow!("MATCH: {e}"))?;
    let rows: Vec<Vec<KuzuValue>> = result.collect();
    assert_eq!(rows.len(), 2, "Expected 2 messages, got {}", rows.len());
    Ok(())
}

/// Verify that parameterized queries work via the cxx bridge.
///
/// Parameterized queries involve an extra FFI crossing to pass parameter
/// values from Rust into the C++ prepared statement executor.
/// This is the same path used by `graph_rows()` with non-empty params.
#[test]
fn kuzu_ffi_parameterized_query_executes() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;

    conn.query("CREATE NODE TABLE IF NOT EXISTS Tag(id STRING, label STRING, PRIMARY KEY (id))")
        .map_err(|e| anyhow::anyhow!("schema: {e}"))?;
    conn.query("CREATE (:Tag {id: 'a', label: 'alpha'})")
        .map_err(|e| anyhow::anyhow!("insert a: {e}"))?;
    conn.query("CREATE (:Tag {id: 'b', label: 'beta'})")
        .map_err(|e| anyhow::anyhow!("insert b: {e}"))?;

    let mut prepared = conn
        .prepare("MATCH (t:Tag {label: $label}) RETURN t.id")
        .map_err(|e| anyhow::anyhow!("prepare: {e}"))?;
    let result = conn
        .execute(
            &mut prepared,
            vec![("label", KuzuValue::String("alpha".to_string()))],
        )
        .map_err(|e| anyhow::anyhow!("execute: {e}"))?;
    let rows: Vec<Vec<KuzuValue>> = result.collect();
    assert_eq!(
        rows.len(),
        1,
        "Expected 1 result for label='alpha', got {}",
        rows.len()
    );
    Ok(())
}

/// Verify that the full KUZU_BACKEND_SCHEMA initializes in a fresh database.
///
/// This is the most comprehensive kuzu FFI smoke test.  It runs all the DDL
/// statements used by the memory backend in production, including all node
/// tables (Session, Agent, EpisodicMemory, SemanticMemory, ProceduralMemory,
/// ProspectiveMemory, WorkingMemory) and all relationship tables.
///
/// If this test fails with linker errors, run:
///   `cargo update -p cxx-build --precise 1.0.138`
#[test]
fn kuzu_ffi_full_backend_schema_initializes() -> Result<()> {
    use anyhow::Context;
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;

    init_graph_backend_schema(&conn).context(
        "KUZU_BACKEND_SCHEMA initialization failed.\n\
        This may indicate a cxx/cxx-build version mismatch.\n\
        Fix: cargo update -p cxx-build --precise 1.0.138\n\
        See docs/howto/resolve-kuzu-linker-errors.md",
    )?;
    Ok(())
}

/// Verify that the generic graph row helper works with an empty params list.
///
/// The `params.is_empty()` branch of `graph_rows()` uses the simpler
/// `conn.query()` path instead of prepare+execute.
#[test]
fn kuzu_rows_helper_no_params() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = graph_rows(&conn, "RETURN 42", vec![])?;
    assert_eq!(rows.len(), 1, "RETURN 42 must produce one row");
    Ok(())
}

/// Verify that the generic graph row helper works with a non-empty params list.
///
/// The parameterized branch of `graph_rows()` prepares the statement and
/// calls execute() with the provided key-value parameters.
#[test]
fn kuzu_rows_helper_with_params() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;

    conn.query("CREATE NODE TABLE IF NOT EXISTS Kv(k STRING, v STRING, PRIMARY KEY (k))")
        .map_err(|e| anyhow::anyhow!("schema: {e}"))?;
    conn.query("CREATE (:Kv {k: 'key1', v: 'val1'})")
        .map_err(|e| anyhow::anyhow!("insert: {e}"))?;

    let rows = graph_rows(
        &conn,
        "MATCH (n:Kv {k: $k}) RETURN n.v",
        vec![("k", KuzuValue::String("key1".to_string()))],
    )?;
    assert_eq!(rows.len(), 1, "Expected 1 row for k='key1'");
    Ok(())
}

/// Verify that the generic graph session lister returns empty list for fresh DB.
///
/// A freshly initialized schema must contain zero sessions.
#[test]
fn kuzu_list_sessions_empty_on_fresh_db() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
    init_graph_backend_schema(&conn)?;

    let sessions = list_graph_sessions_from_conn(&conn)?;
    assert!(
        sessions.is_empty(),
        "Fresh database must have zero sessions, got {}",
        sessions.len()
    );
    Ok(())
}

/// Verify that a session node can be created and then listed.
///
/// This exercises the full Session → EpisodicMemory relationship path
/// that the memory commands use in production.
#[test]
fn kuzu_session_create_and_list() -> Result<()> {
    let (_dir, db) = temp_kuzu_db()?;
    let conn = KuzuConn::new(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
    init_graph_backend_schema(&conn)?;

    let now = "2026-01-02T03:04:05";
    conn.query(&format!(
        "CREATE (:Session {{session_id: 'sess-test-001', start_time: timestamp('{now}'), end_time: timestamp('{now}'), user_id: 'test-user', context: '', status: 'active', created_at: timestamp('{now}'), last_accessed: timestamp('{now}'), metadata: '{{}}'}})"
    ))
    .map_err(|e| anyhow::anyhow!("CREATE Session: {e}"))?;

    let sessions = list_graph_sessions_from_conn(&conn)?;
    assert_eq!(
        sessions.len(),
        1,
        "Expected 1 session after insert, got {}",
        sessions.len()
    );
    assert_eq!(sessions[0].session_id, "sess-test-001");
    assert_eq!(
        sessions[0].memory_count, 0,
        "New session must have 0 memories"
    );
    Ok(())
}
