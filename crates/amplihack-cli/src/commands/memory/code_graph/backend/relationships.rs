use super::super::super::backend::graph_db::{GraphDbConnection, GraphDbValue, graph_rows};
use super::super::BlarifyRelationship;
use super::relationship_exists;
use anyhow::Result;

pub(super) fn import_relationships(
    conn: &GraphDbConnection<'_>,
    relationships: &[BlarifyRelationship],
) -> Result<usize> {
    let mut imported = 0usize;

    for relationship in relationships {
        if relationship.source_id.trim().is_empty() || relationship.target_id.trim().is_empty() {
            continue;
        }

        imported += match relationship.relationship_type.as_str() {
            "CALLS" => {
                create_calls_relationship(conn, &relationship.source_id, &relationship.target_id)?
            }
            "INHERITS" => create_inherits_relationship(
                conn,
                &relationship.source_id,
                &relationship.target_id,
            )?,
            "REFERENCES" => create_references_relationship(
                conn,
                &relationship.source_id,
                &relationship.target_id,
            )?,
            _ => 0,
        };
    }

    Ok(imported)
}

fn create_calls_relationship(
    conn: &GraphDbConnection<'_>,
    source_id: &str,
    target_id: &str,
) -> Result<usize> {
    if relationship_exists(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id})-[r:CALLS]->(target:CodeFunction {function_id: $target_id}) RETURN COUNT(r)",
        vec![
            ("source_id", GraphDbValue::String(source_id.to_string())),
            ("target_id", GraphDbValue::String(target_id.to_string())),
        ],
    )? {
        return Ok(0);
    }

    graph_rows(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id}) MATCH (target:CodeFunction {function_id: $target_id}) CREATE (source)-[:CALLS {call_count: $call_count, context: $context}]->(target)",
        vec![
            ("source_id", GraphDbValue::String(source_id.to_string())),
            ("target_id", GraphDbValue::String(target_id.to_string())),
            ("call_count", GraphDbValue::Int64(1)),
            ("context", GraphDbValue::String(String::new())),
        ],
    )?;
    Ok(1)
}

fn create_inherits_relationship(
    conn: &GraphDbConnection<'_>,
    source_id: &str,
    target_id: &str,
) -> Result<usize> {
    if relationship_exists(
        conn,
        "MATCH (source:CodeClass {class_id: $source_id})-[r:INHERITS]->(target:CodeClass {class_id: $target_id}) RETURN COUNT(r)",
        vec![
            ("source_id", GraphDbValue::String(source_id.to_string())),
            ("target_id", GraphDbValue::String(target_id.to_string())),
        ],
    )? {
        return Ok(0);
    }

    graph_rows(
        conn,
        "MATCH (source:CodeClass {class_id: $source_id}) MATCH (target:CodeClass {class_id: $target_id}) CREATE (source)-[:INHERITS {inheritance_order: $inheritance_order, inheritance_type: $inheritance_type}]->(target)",
        vec![
            ("source_id", GraphDbValue::String(source_id.to_string())),
            ("target_id", GraphDbValue::String(target_id.to_string())),
            ("inheritance_order", GraphDbValue::Int64(0)),
            (
                "inheritance_type",
                GraphDbValue::String("single".to_string()),
            ),
        ],
    )?;
    Ok(1)
}

fn create_references_relationship(
    conn: &GraphDbConnection<'_>,
    source_id: &str,
    target_id: &str,
) -> Result<usize> {
    if relationship_exists(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id})-[r:REFERENCES_CLASS]->(target:CodeClass {class_id: $target_id}) RETURN COUNT(r)",
        vec![
            ("source_id", GraphDbValue::String(source_id.to_string())),
            ("target_id", GraphDbValue::String(target_id.to_string())),
        ],
    )? {
        return Ok(0);
    }

    graph_rows(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id}) MATCH (target:CodeClass {class_id: $target_id}) CREATE (source)-[:REFERENCES_CLASS {reference_type: $reference_type, context: $context}]->(target)",
        vec![
            ("source_id", GraphDbValue::String(source_id.to_string())),
            ("target_id", GraphDbValue::String(target_id.to_string())),
            ("reference_type", GraphDbValue::String("usage".to_string())),
            ("context", GraphDbValue::String(String::new())),
        ],
    )?;
    Ok(1)
}
