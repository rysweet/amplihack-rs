//! Native regression contracts, reached by the existing lbug_ffi test target.
//! Run with --test-threads=1 under the admitted native runner, after compiling
//! through the cache wrapper. These tests also protect ordinary source mode.
//! They do not, by themselves, establish a cache hit or native linking evidence.
use super::{LbugConn, LbugDb, LbugValue, bounded_lbug_config};
use anyhow::Result;

#[test]
fn bounded_native_database_persists_across_close_and_reopen() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("persist.lbug");
    {
        let db = LbugDb::new(&path, bounded_lbug_config())?;
        let conn = LbugConn::new(&db)?;
        conn.query("CREATE NODE TABLE CacheEvidence(id INT64, PRIMARY KEY(id))")?;
        conn.query("CREATE (:CacheEvidence {id: 1917})")?;
    }
    let db = LbugDb::new(&path, bounded_lbug_config())?;
    let conn = LbugConn::new(&db)?;
    let rows: Vec<Vec<LbugValue>> = conn.query("MATCH (n:CacheEvidence) RETURN n.id")?.collect();
    assert!(matches!(rows.as_slice(), [row] if matches!(row.as_slice(), [LbugValue::Int64(1917)])));
    Ok(())
}

#[test]
fn bounded_native_connection_recovers_after_invalid_query() -> Result<()> {
    let (_dir, db) = super::temp_lbug_db()?;
    let conn = LbugConn::new(&db)?;
    assert!(conn.query("THIS IS NOT A VALID CYPHER QUERY").is_err());
    let rows: Vec<Vec<LbugValue>> = conn.query("RETURN 1917")?.collect();
    assert!(matches!(rows.as_slice(), [row] if matches!(row.as_slice(), [LbugValue::Int64(1917)])));
    Ok(())
}
