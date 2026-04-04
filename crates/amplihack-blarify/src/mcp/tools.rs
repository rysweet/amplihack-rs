//! MCP tool wrapper trait for adapting analysis tools to the MCP protocol.
//!
//! Mirrors the Python `mcp_server/tools/base.py`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// JSON Schema type for tool parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonSchemaType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}

/// A single property in a JSON schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaProperty {
    #[serde(rename = "type")]
    pub schema_type: JsonSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaProperty>>,
}

/// MCP input schema definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: HashMap<String, SchemaProperty>,
    #[serde(default)]
    pub required: Vec<String>,
}

impl Default for McpInputSchema {
    fn default() -> Self {
        Self {
            schema_type: "object".into(),
            properties: HashMap::new(),
            required: Vec::new(),
        }
    }
}

/// MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: McpInputSchema,
}

/// Trait for wrapping analysis tools as MCP-compatible tools.
pub trait McpToolWrapper: Send + Sync {
    /// Get the tool name.
    fn name(&self) -> &str;

    /// Get the tool description.
    fn description(&self) -> &str;

    /// Get the MCP input schema.
    fn input_schema(&self) -> McpInputSchema;

    /// Invoke the tool with the given arguments.
    fn invoke(&self, arguments: &HashMap<String, serde_json::Value>) -> Result<serde_json::Value>;

    /// Get the complete MCP tool definition.
    fn to_tool_definition(&self) -> McpToolDefinition {
        McpToolDefinition {
            name: self.name().into(),
            description: self.description().into(),
            input_schema: self.input_schema(),
        }
    }
}

/// Map a Rust type name to a JSON schema type.
pub fn rust_type_to_schema(type_name: &str) -> JsonSchemaType {
    match type_name {
        "String" | "&str" | "str" => JsonSchemaType::String,
        "i32" | "i64" | "u32" | "u64" | "usize" | "isize" => JsonSchemaType::Integer,
        "f32" | "f64" => JsonSchemaType::Number,
        "bool" => JsonSchemaType::Boolean,
        _ if type_name.starts_with("Vec<") => JsonSchemaType::Array,
        _ if type_name.starts_with("HashMap<") => JsonSchemaType::Object,
        _ => JsonSchemaType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_property_serialization() {
        let prop = SchemaProperty {
            schema_type: JsonSchemaType::String,
            description: Some("A test property".into()),
            default: None,
            items: None,
        };
        let json = serde_json::to_string(&prop).unwrap();
        assert!(json.contains("\"type\":\"string\""));
        assert!(json.contains("A test property"));
    }

    #[test]
    fn mcp_input_schema_default() {
        let schema = McpInputSchema::default();
        assert_eq!(schema.schema_type, "object");
        assert!(schema.properties.is_empty());
        assert!(schema.required.is_empty());
    }

    #[test]
    fn tool_definition_roundtrip() {
        let def = McpToolDefinition {
            name: "find_symbols".into(),
            description: "Find code symbols".into(),
            input_schema: McpInputSchema::default(),
        };
        let json = serde_json::to_string(&def).unwrap();
        let deser: McpToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.name, "find_symbols");
    }

    #[test]
    fn rust_type_mapping() {
        assert!(matches!(
            rust_type_to_schema("String"),
            JsonSchemaType::String
        ));
        assert!(matches!(
            rust_type_to_schema("i64"),
            JsonSchemaType::Integer
        ));
        assert!(matches!(rust_type_to_schema("f64"), JsonSchemaType::Number));
        assert!(matches!(
            rust_type_to_schema("bool"),
            JsonSchemaType::Boolean
        ));
        assert!(matches!(
            rust_type_to_schema("Vec<String>"),
            JsonSchemaType::Array
        ));
    }

    #[test]
    fn array_schema_with_items() {
        let prop = SchemaProperty {
            schema_type: JsonSchemaType::Array,
            description: None,
            default: None,
            items: Some(Box::new(SchemaProperty {
                schema_type: JsonSchemaType::String,
                description: None,
                default: None,
                items: None,
            })),
        };
        let json = serde_json::to_string(&prop).unwrap();
        assert!(json.contains("\"items\""));
    }
}
