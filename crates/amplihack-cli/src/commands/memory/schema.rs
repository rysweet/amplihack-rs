pub(crate) const GRAPH_DB_TREE_BACKEND_NAME: &str = "graph-db";
pub(crate) const HIERARCHICAL_SCHEMA: &[&str] = &[
    r#"CREATE NODE TABLE IF NOT EXISTS SemanticMemory(
        memory_id STRING,
        concept STRING,
        content STRING,
        confidence DOUBLE,
        source_id STRING,
        agent_id STRING,
        tags STRING,
        metadata STRING,
        created_at STRING,
        entity_name STRING DEFAULT '',
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS EpisodicMemory(
        memory_id STRING,
        content STRING,
        source_label STRING,
        agent_id STRING,
        tags STRING,
        metadata STRING,
        created_at STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS SIMILAR_TO(
        FROM SemanticMemory TO SemanticMemory,
        weight DOUBLE,
        metadata STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS DERIVES_FROM(
        FROM SemanticMemory TO EpisodicMemory,
        extraction_method STRING,
        confidence DOUBLE
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS SUPERSEDES(
        FROM SemanticMemory TO SemanticMemory,
        reason STRING,
        temporal_delta STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS TRANSITIONED_TO(
        FROM SemanticMemory TO SemanticMemory,
        from_value STRING,
        to_value STRING,
        turn INT64,
        transition_type STRING
    )"#,
];
