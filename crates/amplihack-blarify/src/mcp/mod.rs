//! MCP (Model Context Protocol) server for blarify analysis tools.
//!
//! Provides the server, configuration, and tool wrapper trait.

pub mod config;
pub mod server;
pub mod tools;

pub use config::{DbType, McpServerConfig};
pub use server::BlarifyMcpServer;
pub use tools::{McpInputSchema, McpToolDefinition, McpToolWrapper};
