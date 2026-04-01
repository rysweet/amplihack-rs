use super::super::super::*;
use super::{GraphDbConnection, GraphDbValue};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

pub fn graph_rows(
    conn: &GraphDbConnection<'_>,
    query: &str,
    params: Vec<(&str, GraphDbValue)>,
) -> Result<Vec<Vec<GraphDbValue>>> {
    if params.is_empty() {
        return Ok(conn.query(query)?.collect());
    }
    let mut prepared = conn.prepare(query)?;
    Ok(conn.execute(&mut prepared, params)?.collect())
}

pub(crate) fn memory_from_graph_node(
    value: &GraphDbValue,
    _session_id: &str,
    label: &str,
) -> Result<MemoryRecord> {
    let props = match value {
        GraphDbValue::Node(node) => node.get_properties(),
        other => anyhow::bail!("expected graph node, got {other}"),
    };
    let metadata = property_string(props, "metadata")
        .as_deref()
        .map(parse_json_value)
        .transpose()?
        .unwrap_or(JsonValue::Object(Default::default()));
    let importance = property_i64(props, "importance")
        .or_else(|| property_i64(props, "importance_score"))
        .or_else(|| property_i64(props, "priority"));
    Ok(MemoryRecord {
        memory_id: property_string(props, "memory_id").unwrap_or_default(),
        memory_type: label
            .strip_suffix("Memory")
            .unwrap_or(label)
            .to_ascii_lowercase(),
        title: property_string(props, "title")
            .or_else(|| property_string(props, "concept"))
            .or_else(|| property_string(props, "procedure_name"))
            .unwrap_or_default(),
        content: property_string(props, "content").unwrap_or_default(),
        metadata,
        importance,
        accessed_at: property_string(props, "accessed_at"),
        expires_at: property_string(props, "expires_at"),
    })
}

pub(crate) fn property_string(props: &[(String, GraphDbValue)], key: &str) -> Option<String> {
    props.iter().find_map(|(name, value)| {
        if name == key {
            Some(graph_value_to_string(value))
        } else {
            None
        }
    })
}

pub(crate) fn property_i64(props: &[(String, GraphDbValue)], key: &str) -> Option<i64> {
    props.iter().find_map(|(name, value)| {
        if name == key {
            graph_value_to_i64(value)
        } else {
            None
        }
    })
}

pub(crate) fn graph_value_to_string(value: &GraphDbValue) -> String {
    match value {
        GraphDbValue::Null(_) => String::new(),
        GraphDbValue::String(v) => v.clone(),
        GraphDbValue::Timestamp(v) => {
            DateTime::<Utc>::from_timestamp(v.unix_timestamp(), v.nanosecond())
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| v.to_string())
        }
        other => other.to_string(),
    }
}

pub(crate) fn graph_value_to_i64(value: &GraphDbValue) -> Option<i64> {
    match value {
        GraphDbValue::Int64(v) => Some(*v),
        GraphDbValue::Int32(v) => Some(i64::from(*v)),
        GraphDbValue::Int16(v) => Some(i64::from(*v)),
        GraphDbValue::Int8(v) => Some(i64::from(*v)),
        GraphDbValue::UInt64(v) => i64::try_from(*v).ok(),
        GraphDbValue::UInt32(v) => Some(i64::from(*v)),
        GraphDbValue::UInt16(v) => Some(i64::from(*v)),
        GraphDbValue::UInt8(v) => Some(i64::from(*v)),
        GraphDbValue::Double(v) => Some(*v as i64),
        GraphDbValue::Float(v) => Some(*v as i64),
        _ => None,
    }
}

pub(crate) fn graph_string(value: Option<&GraphDbValue>) -> Result<String> {
    Ok(value.map(graph_value_to_string).unwrap_or_default())
}

pub(crate) fn graph_i64(value: Option<&GraphDbValue>) -> Result<i64> {
    value
        .and_then(graph_value_to_i64)
        .context("expected integer graph value")
}

pub(crate) fn graph_f64(value: Option<&GraphDbValue>) -> Result<f64> {
    match value {
        Some(GraphDbValue::Double(v)) => Ok(*v),
        Some(GraphDbValue::Float(v)) => Ok(f64::from(*v)),
        Some(GraphDbValue::Int64(v)) => Ok(*v as f64),
        Some(GraphDbValue::Int32(v)) => Ok(f64::from(*v)),
        Some(GraphDbValue::UInt64(v)) => Ok(*v as f64),
        Some(GraphDbValue::UInt32(v)) => Ok(f64::from(*v)),
        Some(GraphDbValue::Null(_)) | None => Ok(0.0),
        Some(other) => anyhow::bail!("expected numeric graph value, got {other}"),
    }
}
