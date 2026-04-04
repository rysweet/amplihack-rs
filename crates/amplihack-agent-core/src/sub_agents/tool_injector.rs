//! Tool Injector: SDK-specific tool capability injection.
//!
//! Provides pre-defined tool descriptors for each SDK backend. These are
//! schema-only definitions — actual execution is handled by the SDK runtime.
//! Mirrors Python `tool_injector.py`.

use serde_json::json;

use crate::sdk_adapters::types::{AgentTool, SdkType, ToolCategory};

// ---------------------------------------------------------------------------
// SDK tool definitions
// ---------------------------------------------------------------------------

fn claude_tools() -> Vec<AgentTool> {
    vec![
        AgentTool::new("bash", "Execute a bash command in a sandboxed environment")
            .with_parameter(
                "command",
                json!({"type": "string", "description": "The bash command to execute"}),
            )
            .with_category(ToolCategory::Custom),
        AgentTool::new("read_file", "Read the contents of a file")
            .with_parameter(
                "path",
                json!({"type": "string", "description": "File path to read"}),
            )
            .with_category(ToolCategory::Custom),
        AgentTool::new("write_file", "Write content to a file")
            .with_parameter(
                "path",
                json!({"type": "string", "description": "File path to write"}),
            )
            .with_parameter(
                "content",
                json!({"type": "string", "description": "Content to write"}),
            )
            .with_category(ToolCategory::Custom),
        AgentTool::new("edit_file", "Edit a file with search and replace")
            .with_parameter(
                "path",
                json!({"type": "string", "description": "File path to edit"}),
            )
            .with_parameter(
                "old_text",
                json!({"type": "string", "description": "Text to find"}),
            )
            .with_parameter(
                "new_text",
                json!({"type": "string", "description": "Replacement text"}),
            )
            .with_category(ToolCategory::Custom),
    ]
}

fn copilot_tools() -> Vec<AgentTool> {
    vec![
        AgentTool::new("file_system", "Read, write, or list files and directories")
            .with_parameter(
                "operation",
                json!({
                    "type": "string",
                    "enum": ["read", "write", "list"],
                    "description": "File system operation"
                }),
            )
            .with_parameter(
                "path",
                json!({"type": "string", "description": "File or directory path"}),
            )
            .with_parameter(
                "content",
                json!({"type": "string", "description": "Content for write operations"}),
            )
            .with_category(ToolCategory::Custom),
        AgentTool::new("git", "Execute git operations")
            .with_parameter(
                "command",
                json!({"type": "string", "description": "Git command to execute"}),
            )
            .with_category(ToolCategory::Custom),
        AgentTool::new("web_requests", "Make HTTP requests to external services")
            .with_parameter(
                "url",
                json!({"type": "string", "description": "URL to request"}),
            )
            .with_parameter(
                "method",
                json!({
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "DELETE"],
                    "default": "GET"
                }),
            )
            .with_category(ToolCategory::Custom),
    ]
}

fn microsoft_tools() -> Vec<AgentTool> {
    vec![
        AgentTool::new(
            "agent_execute",
            "Execute a task through the Microsoft Agent Framework",
        )
        .with_parameter(
            "task",
            json!({"type": "string", "description": "Task to execute"}),
        )
        .with_parameter(
            "context",
            json!({"type": "string", "description": "Additional context"}),
        )
        .with_category(ToolCategory::Custom),
        AgentTool::new("agent_query", "Query the agent framework for information")
            .with_parameter(
                "query",
                json!({"type": "string", "description": "Query to run"}),
            )
            .with_category(ToolCategory::Custom),
    ]
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get the native tool definitions for an SDK type.
pub fn get_sdk_tools(sdk_type: SdkType) -> Vec<AgentTool> {
    match sdk_type {
        SdkType::Claude => claude_tools(),
        SdkType::Copilot => copilot_tools(),
        SdkType::Microsoft => microsoft_tools(),
        SdkType::Mini => Vec::new(),
    }
}

/// Get the native tool names for an SDK type.
pub fn get_sdk_tool_names(sdk_type: SdkType) -> Vec<String> {
    get_sdk_tools(sdk_type)
        .into_iter()
        .map(|t| t.name)
        .collect()
}

/// Inject SDK-specific tools into a tool list, skipping duplicates.
///
/// Returns the number of tools injected.
///
/// # Example
///
/// ```
/// use amplihack_agent_core::sub_agents::inject_sdk_tools;
/// use amplihack_agent_core::sdk_adapters::types::{AgentTool, SdkType};
///
/// let mut tools = vec![AgentTool::new("bash", "existing bash tool")];
/// let count = inject_sdk_tools(&mut tools, SdkType::Claude);
/// // "bash" already existed, so only 3 new tools injected
/// assert_eq!(count, 3);
/// ```
pub fn inject_sdk_tools(tools: &mut Vec<AgentTool>, sdk_type: SdkType) -> usize {
    let sdk_tools = get_sdk_tools(sdk_type);
    if sdk_tools.is_empty() {
        return 0;
    }

    let existing: std::collections::HashSet<String> =
        tools.iter().map(|t| t.name.clone()).collect();
    let mut injected = 0;

    for tool in sdk_tools {
        if !existing.contains(&tool.name) {
            tools.push(tool);
            injected += 1;
        }
    }

    injected
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_has_four_tools() {
        let tools = get_sdk_tools(SdkType::Claude);
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
    }

    #[test]
    fn copilot_has_three_tools() {
        let tools = get_sdk_tools(SdkType::Copilot);
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"file_system"));
        assert!(names.contains(&"git"));
        assert!(names.contains(&"web_requests"));
    }

    #[test]
    fn microsoft_has_two_tools() {
        let tools = get_sdk_tools(SdkType::Microsoft);
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn mini_has_no_tools() {
        let tools = get_sdk_tools(SdkType::Mini);
        assert!(tools.is_empty());
    }

    #[test]
    fn tool_names_match_definitions() {
        let names = get_sdk_tool_names(SdkType::Claude);
        assert_eq!(names, vec!["bash", "read_file", "write_file", "edit_file"]);
    }

    #[test]
    fn inject_deduplicates() {
        let mut tools = vec![AgentTool::new("bash", "existing")];
        let count = inject_sdk_tools(&mut tools, SdkType::Claude);
        assert_eq!(count, 3); // bash already exists
        assert_eq!(tools.len(), 4); // 1 existing + 3 new
    }

    #[test]
    fn inject_all_new() {
        let mut tools = Vec::new();
        let count = inject_sdk_tools(&mut tools, SdkType::Copilot);
        assert_eq!(count, 3);
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn inject_mini_returns_zero() {
        let mut tools = Vec::new();
        let count = inject_sdk_tools(&mut tools, SdkType::Mini);
        assert_eq!(count, 0);
        assert!(tools.is_empty());
    }

    #[test]
    fn tools_have_parameters() {
        let tools = get_sdk_tools(SdkType::Claude);
        let bash = &tools[0];
        assert!(bash.parameters.contains_key("command"));
    }

    #[test]
    fn tools_serde_roundtrip() {
        let tools = get_sdk_tools(SdkType::Claude);
        let json = serde_json::to_string(&tools).unwrap();
        let parsed: Vec<AgentTool> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].name, "bash");
    }

    #[test]
    fn inject_preserves_existing_tools() {
        let mut tools = vec![
            AgentTool::new("custom_tool", "my custom tool"),
            AgentTool::new("another", "another tool"),
        ];
        inject_sdk_tools(&mut tools, SdkType::Microsoft);
        assert_eq!(tools.len(), 4); // 2 existing + 2 microsoft
        assert_eq!(tools[0].name, "custom_tool");
        assert_eq!(tools[1].name, "another");
    }
}
