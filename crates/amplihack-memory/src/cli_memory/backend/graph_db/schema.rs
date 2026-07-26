use super::GraphDbConnection;
use anyhow::Result;

pub(crate) const GRAPH_BACKEND_SCHEMA: &[&str] = &[
    r#"CREATE NODE TABLE IF NOT EXISTS Session(
        session_id STRING,
        start_time TIMESTAMP,
        end_time TIMESTAMP,
        user_id STRING,
        context STRING,
        status STRING,
        created_at TIMESTAMP,
        last_accessed TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (session_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS Agent(
        agent_id STRING,
        name STRING,
        first_used TIMESTAMP,
        last_used TIMESTAMP,
        PRIMARY KEY (agent_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS EpisodicMemory(
        memory_id STRING,
        timestamp TIMESTAMP,
        content STRING,
        event_type STRING,
        emotional_valence DOUBLE,
        importance_score DOUBLE,
        title STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        expires_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS SemanticMemory(
        memory_id STRING,
        concept STRING,
        content STRING,
        category STRING,
        confidence_score DOUBLE,
        last_updated TIMESTAMP,
        version INT64,
        title STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS ProceduralMemory(
        memory_id STRING,
        procedure_name STRING,
        description STRING,
        steps STRING,
        preconditions STRING,
        postconditions STRING,
        success_rate DOUBLE,
        usage_count INT64,
        last_used TIMESTAMP,
        title STRING,
        content STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS ProspectiveMemory(
        memory_id STRING,
        intention STRING,
        trigger_condition STRING,
        priority STRING,
        due_date TIMESTAMP,
        status STRING,
        scope STRING,
        completion_criteria STRING,
        title STRING,
        content STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        expires_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS WorkingMemory(
        memory_id STRING,
        content STRING,
        memory_type STRING,
        priority INT64,
        created_at TIMESTAMP,
        ttl_seconds INT64,
        title STRING,
        metadata STRING,
        tags STRING,
        accessed_at TIMESTAMP,
        expires_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS CONTAINS_EPISODIC(FROM Session TO EpisodicMemory, sequence_number INT64)"#,
    r#"CREATE REL TABLE IF NOT EXISTS CONTAINS_WORKING(FROM Session TO WorkingMemory, activation_level DOUBLE)"#,
    r#"CREATE REL TABLE IF NOT EXISTS CONTRIBUTES_TO_SEMANTIC(FROM Session TO SemanticMemory, contribution_type STRING, timestamp TIMESTAMP, delta STRING)"#,
    r#"CREATE REL TABLE IF NOT EXISTS USES_PROCEDURE(FROM Session TO ProceduralMemory, timestamp TIMESTAMP, success BOOL, notes STRING)"#,
    r#"CREATE REL TABLE IF NOT EXISTS CREATES_INTENTION(FROM Session TO ProspectiveMemory, timestamp TIMESTAMP)"#,
];

/// `(node_label, relationship_name, has_expires_at)`.
///
/// `has_expires_at` is true when the node table includes an `expires_at`
/// TIMESTAMP column in the schema.  SemanticMemory and ProceduralMemory
/// represent long-lived facts with no TTL, so they never expire.
pub(crate) const GRAPH_MEMORY_TABLES: &[(&str, &str, bool)] = &[
    ("EpisodicMemory", "CONTAINS_EPISODIC", true),
    ("SemanticMemory", "CONTRIBUTES_TO_SEMANTIC", false),
    ("ProceduralMemory", "USES_PROCEDURE", false),
    ("ProspectiveMemory", "CREATES_INTENTION", true),
    ("WorkingMemory", "CONTAINS_WORKING", true),
];

pub fn init_graph_backend_schema(conn: &GraphDbConnection<'_>) -> Result<()> {
    for statement in GRAPH_BACKEND_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}
