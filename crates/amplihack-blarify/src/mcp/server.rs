//! MCP server for exposing blarify analysis tools.
//!
//! Mirrors the Python `mcp_server/server.py`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use tracing::{debug, info};

use super::config::McpServerConfig;
use super::tools::{McpToolDefinition, McpToolWrapper};

/// MCP server that wraps blarify analysis tools.
pub struct BlarifyMcpServer {
    config: McpServerConfig,
    tools: HashMap<String, Box<dyn McpToolWrapper>>,
}

impl BlarifyMcpServer {
    /// Create a new MCP server with configuration.
    pub fn new(config: McpServerConfig) -> Self {
        info!(
            root_path = %config.root_path,
            db_type = ?config.db_type,
            "Initializing BlarifyMcpServer"
        );
        Self {
            config,
            tools: HashMap::new(),
        }
    }

    /// Register a tool with the server.
    pub fn register_tool(&mut self, tool: Box<dyn McpToolWrapper>) {
        let name = tool.name().to_string();
        debug!(tool = %name, "Registering MCP tool");
        self.tools.insert(name, tool);
    }

    /// List all registered tool definitions.
    pub fn list_tools(&self) -> Vec<McpToolDefinition> {
        self.tools
            .values()
            .map(|t| t.to_tool_definition())
            .collect()
    }

    /// Invoke a tool by name with arguments.
    pub fn invoke_tool(
        &self,
        name: &str,
        arguments: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let tool = self
            .tools
            .get(name)
            .with_context(|| format!("Tool not found: {name}"))?;
        tool.invoke(arguments)
    }

    /// Get the server configuration.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Get count of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::{JsonSchemaType, McpInputSchema, SchemaProperty};

    struct DummyTool;

    impl McpToolWrapper for DummyTool {
        fn name(&self) -> &str {
            "dummy_tool"
        }
        fn description(&self) -> &str {
            "A dummy test tool"
        }
        fn input_schema(&self) -> McpInputSchema {
            let mut props = HashMap::new();
            props.insert(
                "query".into(),
                SchemaProperty {
                    schema_type: JsonSchemaType::String,
                    description: Some("Search query".into()),
                    default: None,
                    items: None,
                },
            );
            McpInputSchema {
                schema_type: "object".into(),
                properties: props,
                required: vec!["query".into()],
            }
        }
        fn invoke(
            &self,
            arguments: &HashMap<String, serde_json::Value>,
        ) -> Result<serde_json::Value> {
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("none");
            Ok(serde_json::json!({"result": format!("found: {query}")}))
        }
    }

    fn make_config() -> McpServerConfig {
        McpServerConfig {
            neo4j_uri: "bolt://localhost:7687".into(),
            neo4j_username: "neo4j".into(),
            neo4j_password: "pass".into(),
            root_path: "/repo".into(),
            entity_id: "default".into(),
            db_type: super::super::config::DbType::Neo4j,
            falkor_host: None,
            falkor_port: None,
        }
    }

    #[test]
    fn server_register_and_list() {
        let mut server = BlarifyMcpServer::new(make_config());
        server.register_tool(Box::new(DummyTool));
        assert_eq!(server.tool_count(), 1);

        let tools = server.list_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "dummy_tool");
    }

    #[test]
    fn server_invoke_tool() {
        let mut server = BlarifyMcpServer::new(make_config());
        server.register_tool(Box::new(DummyTool));

        let mut args = HashMap::new();
        args.insert("query".into(), serde_json::Value::String("test".into()));
        let result = server.invoke_tool("dummy_tool", &args).unwrap();
        assert_eq!(result["result"], "found: test");
    }

    #[test]
    fn server_invoke_unknown_tool() {
        let server = BlarifyMcpServer::new(make_config());
        let result = server.invoke_tool("nonexistent", &HashMap::new());
        assert!(result.is_err());
    }
}
