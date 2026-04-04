// When the `graph-db` feature is disabled, the parent module
// (`backend/mod.rs`) provides an inline stub module instead of loading
// this directory module.  All items below are therefore only compiled
// when lbug is available.

mod handle;
mod learning;
mod queries;
mod resolve;
mod schema;
mod values;

#[cfg(test)]
pub(crate) use lbug::LogicalType as GraphDbLogicalType;
pub(crate) use lbug::{
    Connection as GraphDbConnection, Database as GraphDbDatabase,
    SystemConfig as GraphDbSystemConfig, Value as GraphDbValue,
};

pub(crate) use handle::{GraphDbBackend, GraphDbHandle};
pub use queries::list_graph_sessions_from_conn;
pub(crate) use resolve::resolve_memory_graph_db_path;
pub use schema::init_graph_backend_schema;
pub use values::graph_rows;
pub(crate) use values::{graph_f64, graph_i64, graph_string};
#[cfg(test)]
pub(crate) use values::{graph_value_to_i64, graph_value_to_string, memory_from_graph_node};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_value_to_string_handles_string_variant() {
        let val = GraphDbValue::String("hello".to_string());
        assert_eq!(graph_value_to_string(&val), "hello");
    }

    #[test]
    fn graph_value_to_string_handles_null() {
        let val = GraphDbValue::Null(GraphDbLogicalType::String);
        assert_eq!(
            graph_value_to_string(&val),
            "",
            "Null must convert to empty string"
        );
    }

    #[test]
    fn graph_value_to_string_handles_non_string_via_display() {
        let val = GraphDbValue::Int64(42);
        let s = graph_value_to_string(&val);
        assert!(
            s.contains("42"),
            "Int64(42) should display as a string containing '42', got: {s}"
        );
    }

    #[test]
    fn graph_value_to_i64_extracts_int64() {
        assert_eq!(graph_value_to_i64(&GraphDbValue::Int64(99)), Some(99));
    }

    #[test]
    fn graph_value_to_i64_extracts_int32() {
        assert_eq!(graph_value_to_i64(&GraphDbValue::Int32(7)), Some(7));
    }

    #[test]
    fn graph_value_to_i64_extracts_uint32() {
        assert_eq!(graph_value_to_i64(&GraphDbValue::UInt32(5)), Some(5));
    }

    #[test]
    fn graph_value_to_i64_returns_none_for_non_numeric() {
        let val = GraphDbValue::String("abc".to_string());
        assert_eq!(
            graph_value_to_i64(&val),
            None,
            "Non-numeric value must return None"
        );
    }

    #[test]
    fn graph_value_to_i64_extracts_double_as_truncated_i64() {
        assert_eq!(graph_value_to_i64(&GraphDbValue::Double(3.9)), Some(3));
    }

    #[test]
    fn memory_from_graph_node_uses_graph_neutral_error_wording() {
        let err = memory_from_graph_node(
            &GraphDbValue::String("oops".to_string()),
            "session-1",
            "SemanticMemory",
        )
        .expect_err("non-node value must fail");
        assert!(err.to_string().contains("expected graph node"));
        assert!(!err.to_string().contains("Kùzu"));
    }

    #[test]
    fn graph_i64_uses_graph_neutral_error_wording() {
        let err = graph_i64(Some(&GraphDbValue::String("oops".to_string())))
            .expect_err("non-integer value must fail");
        assert!(err.to_string().contains("expected integer graph value"));
        assert!(!err.to_string().contains("Kùzu"));
    }

    #[test]
    fn graph_f64_uses_graph_neutral_error_wording() {
        let err = graph_f64(Some(&GraphDbValue::String("oops".to_string())))
            .expect_err("non-numeric value must fail");
        assert!(err.to_string().contains("expected numeric graph value"));
        assert!(!err.to_string().contains("Kùzu"));
    }
}
