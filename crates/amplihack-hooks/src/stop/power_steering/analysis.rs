use super::{TodoItem, ToolResult, ToolUse, TranscriptMessage};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

pub(super) fn read_transcript_messages(path: &Path) -> anyhow::Result<Vec<TranscriptMessage>> {
    let raw = std::fs::read_to_string(path)?;
    let mut messages = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(trimmed)?;
        if let Some(message) = parse_transcript_message(&entry) {
            messages.push(message);
        }
    }

    Ok(messages)
}

fn parse_transcript_message(entry: &Value) -> Option<TranscriptMessage> {
    if let Some(role) = entry.get("role").and_then(Value::as_str) {
        let content = entry.get("content")?;
        return Some(parse_message_content(role, content));
    }

    let message = entry.get("message")?;
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| entry.get("type").and_then(Value::as_str))?;
    Some(parse_message_content(role, message.get("content")?))
}

fn parse_message_content(role: &str, content: &Value) -> TranscriptMessage {
    let mut text_blocks = Vec::new();
    let mut tool_uses = Vec::new();
    let mut tool_results = Vec::new();

    match content {
        Value::String(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                text_blocks.push(trimmed.to_string());
            }
        }
        Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                text_blocks.push(trimmed.to_string());
                            }
                        }
                    }
                    Some("tool_use") => tool_uses.push(ToolUse {
                        id: block.get("id").and_then(Value::as_str).map(str::to_string),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                    }),
                    Some("tool_result") => tool_results.push(ToolResult {
                        tool_use_id: block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        is_error: block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }),
                    _ => {}
                }
            }
        }
        _ => {}
    }

    TranscriptMessage {
        role: role.to_string(),
        text: text_blocks.join("\n"),
        tool_uses,
        tool_results,
    }
}

pub(super) fn is_qa_session(messages: &[TranscriptMessage]) -> bool {
    let tool_calls = messages
        .iter()
        .map(|message| message.tool_uses.len())
        .sum::<usize>();
    if tool_calls > 0 {
        return false;
    }

    let user_messages = messages
        .iter()
        .filter(|message| message.role == "user" && !message.text.is_empty())
        .collect::<Vec<_>>();

    if user_messages.is_empty() {
        return false;
    }

    let question_count = user_messages
        .iter()
        .filter(|message| message.text.contains('?'))
        .count();

    question_count * 2 > user_messages.len()
}

pub(super) fn extract_todo_items(input: &Value) -> Vec<TodoItem> {
    let Some(items) = input
        .get("todos")
        .or_else(|| input.get("items"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let label = item
                .get("content")
                .or_else(|| item.get("text"))
                .or_else(|| item.get("title"))
                .and_then(Value::as_str)?
                .trim()
                .to_string();
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .trim()
                .to_ascii_lowercase();
            Some(TodoItem { label, status })
        })
        .collect()
}

pub(super) fn transcript_has_code_changes(messages: &[TranscriptMessage]) -> bool {
    messages
        .iter()
        .flat_map(|message| message.tool_uses.iter())
        .any(|tool_use| {
            matches!(
                normalize_tool_name(&tool_use.name).as_str(),
                "edit" | "write" | "multiedit" | "apply_patch" | "createfile" | "notebookedit"
            ) && extract_paths_from_input(&tool_use.input)
                .iter()
                .any(|path| is_code_path(path))
        })
}

pub(super) fn has_successful_local_test(messages: &[TranscriptMessage]) -> bool {
    let mut pending_by_id = HashMap::new();
    let mut anonymous_pending = VecDeque::new();

    for message in messages {
        for tool_use in &message.tool_uses {
            let normalized_name = normalize_tool_name(&tool_use.name);
            if normalized_name != "bash" && normalized_name != "terminal" {
                continue;
            }

            let Some(command) = tool_use
                .input
                .get("command")
                .and_then(Value::as_str)
                .map(str::trim)
            else {
                continue;
            };

            if !is_test_command(command) {
                continue;
            }

            if let Some(id) = tool_use.id.clone() {
                pending_by_id.insert(id, ());
            } else {
                anonymous_pending.push_back(());
            }
        }

        for tool_result in &message.tool_results {
            if tool_result.is_error {
                continue;
            }

            if let Some(tool_use_id) = &tool_result.tool_use_id {
                if pending_by_id.remove(tool_use_id).is_some() {
                    return true;
                }
                continue;
            }

            if anonymous_pending.pop_front().is_some() {
                return true;
            }
        }
    }

    false
}

fn is_test_command(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    [
        "cargo test",
        "pytest",
        "uv run pytest",
        "npm test",
        "pnpm test",
        "yarn test",
        "bun test",
        "go test",
        "dotnet test",
        "deno test",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

fn extract_paths_from_input(input: &Value) -> Vec<String> {
    let mut paths = HashSet::new();
    collect_paths_recursive(input, &mut paths);
    paths.into_iter().collect()
}

fn collect_paths_recursive(value: &Value, paths: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if matches!(
                    key.as_str(),
                    "path" | "file_path" | "filePath" | "target_file" | "targetFile"
                ) && let Some(path) = nested.as_str()
                {
                    let trimmed = path.trim();
                    if !trimmed.is_empty() {
                        paths.insert(trimmed.to_string());
                    }
                }
                collect_paths_recursive(nested, paths);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_paths_recursive(item, paths);
            }
        }
        _ => {}
    }
}

fn is_code_path(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    [
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".java", ".kt", ".swift", ".rb", ".php",
        ".c", ".cc", ".cpp", ".h", ".hpp", ".cs",
    ]
    .iter()
    .any(|ext| lowered.ends_with(ext))
}

pub(super) fn first_user_message(messages: &[TranscriptMessage]) -> Option<&str> {
    messages
        .iter()
        .find(|message| message.role == "user" && !message.text.is_empty())
        .map(|message| message.text.as_str())
}

pub(super) fn last_assistant_message(messages: &[TranscriptMessage]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.text.is_empty())
        .map(|message| message.text.as_str())
}

pub(super) fn normalize_tool_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> TranscriptMessage {
        TranscriptMessage {
            role: role.to_string(),
            text: text.to_string(),
            tool_uses: Vec::new(),
            tool_results: Vec::new(),
        }
    }

    fn msg_with_tools(
        role: &str,
        text: &str,
        tool_uses: Vec<ToolUse>,
        tool_results: Vec<ToolResult>,
    ) -> TranscriptMessage {
        TranscriptMessage {
            role: role.to_string(),
            text: text.to_string(),
            tool_uses,
            tool_results,
        }
    }

    // -----------------------------------------------------------------------
    // normalize_tool_name
    // -----------------------------------------------------------------------

    #[test]
    fn normalize_strips_and_lowercases() {
        assert_eq!(normalize_tool_name("  Edit  "), "edit");
    }

    #[test]
    fn normalize_replaces_hyphens() {
        assert_eq!(normalize_tool_name("multi-edit"), "multi_edit");
    }

    #[test]
    fn normalize_combined() {
        assert_eq!(normalize_tool_name(" Apply-Patch "), "apply_patch");
    }

    // -----------------------------------------------------------------------
    // is_qa_session
    // -----------------------------------------------------------------------

    #[test]
    fn qa_session_with_questions_and_no_tools() {
        let msgs = vec![
            msg("user", "What is Rust?"),
            msg("assistant", "Rust is a language."),
        ];
        assert!(is_qa_session(&msgs));
    }

    #[test]
    fn qa_session_false_when_tools_used() {
        let msgs = vec![msg_with_tools(
            "assistant",
            "Looking...",
            vec![ToolUse {
                id: None,
                name: "bash".to_string(),
                input: Value::Null,
            }],
            Vec::new(),
        )];
        assert!(!is_qa_session(&msgs));
    }

    #[test]
    fn qa_session_false_when_no_user_messages() {
        let msgs = vec![msg("assistant", "Hello")];
        assert!(!is_qa_session(&msgs));
    }

    #[test]
    fn qa_session_false_when_few_questions() {
        let msgs = vec![
            msg("user", "Do this."),
            msg("user", "And this."),
            msg("user", "What about this?"),
        ];
        // Only 1/3 have '?' — need > 50%
        assert!(!is_qa_session(&msgs));
    }

    // -----------------------------------------------------------------------
    // extract_todo_items
    // -----------------------------------------------------------------------

    #[test]
    fn extract_todo_from_todos_key() {
        let input = serde_json::json!({
            "todos": [
                {"content": "Write tests", "status": "pending"},
                {"content": "Fix bug", "status": "done"}
            ]
        });
        let items = extract_todo_items(&input);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Write tests");
        assert_eq!(items[0].status, "pending");
        assert_eq!(items[1].status, "done");
    }

    #[test]
    fn extract_todo_from_items_key() {
        let input = serde_json::json!({
            "items": [{"title": "Task A"}]
        });
        let items = extract_todo_items(&input);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Task A");
        assert_eq!(items[0].status, "pending"); // default
    }

    #[test]
    fn extract_todo_empty_on_no_key() {
        let input = serde_json::json!({"other": "data"});
        assert!(extract_todo_items(&input).is_empty());
    }

    // -----------------------------------------------------------------------
    // transcript_has_code_changes
    // -----------------------------------------------------------------------

    #[test]
    fn code_changes_with_edit_tool() {
        let msgs = vec![msg_with_tools(
            "assistant",
            "",
            vec![ToolUse {
                id: None,
                name: "Edit".to_string(),
                input: serde_json::json!({"path": "src/main.rs"}),
            }],
            Vec::new(),
        )];
        assert!(transcript_has_code_changes(&msgs));
    }

    #[test]
    fn code_changes_false_for_non_code_path() {
        let msgs = vec![msg_with_tools(
            "assistant",
            "",
            vec![ToolUse {
                id: None,
                name: "Edit".to_string(),
                input: serde_json::json!({"path": "README.md"}),
            }],
            Vec::new(),
        )];
        assert!(!transcript_has_code_changes(&msgs));
    }

    #[test]
    fn code_changes_false_without_tools() {
        let msgs = vec![msg("assistant", "No tools here")];
        assert!(!transcript_has_code_changes(&msgs));
    }

    // -----------------------------------------------------------------------
    // has_successful_local_test
    // -----------------------------------------------------------------------

    #[test]
    fn successful_test_with_matching_result() {
        let msgs = vec![
            msg_with_tools(
                "assistant",
                "",
                vec![ToolUse {
                    id: Some("t1".into()),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cargo test"}),
                }],
                Vec::new(),
            ),
            msg_with_tools(
                "user",
                "",
                Vec::new(),
                vec![ToolResult {
                    tool_use_id: Some("t1".into()),
                    is_error: false,
                }],
            ),
        ];
        assert!(has_successful_local_test(&msgs));
    }

    #[test]
    fn failed_test_returns_false() {
        let msgs = vec![
            msg_with_tools(
                "assistant",
                "",
                vec![ToolUse {
                    id: Some("t1".into()),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cargo test"}),
                }],
                Vec::new(),
            ),
            msg_with_tools(
                "user",
                "",
                Vec::new(),
                vec![ToolResult {
                    tool_use_id: Some("t1".into()),
                    is_error: true,
                }],
            ),
        ];
        assert!(!has_successful_local_test(&msgs));
    }

    #[test]
    fn no_test_command_returns_false() {
        let msgs = vec![msg_with_tools(
            "assistant",
            "",
            vec![ToolUse {
                id: None,
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls -la"}),
            }],
            Vec::new(),
        )];
        assert!(!has_successful_local_test(&msgs));
    }

    // -----------------------------------------------------------------------
    // first_user_message / last_assistant_message
    // -----------------------------------------------------------------------

    #[test]
    fn first_user_message_found() {
        let msgs = vec![
            msg("assistant", "Hi"),
            msg("user", "Hello there"),
            msg("user", "Another one"),
        ];
        assert_eq!(first_user_message(&msgs), Some("Hello there"));
    }

    #[test]
    fn first_user_message_skips_empty() {
        let msgs = vec![msg("user", ""), msg("user", "Real message")];
        assert_eq!(first_user_message(&msgs), Some("Real message"));
    }

    #[test]
    fn first_user_message_none_when_absent() {
        let msgs = vec![msg("assistant", "Only assistant")];
        assert_eq!(first_user_message(&msgs), None);
    }

    #[test]
    fn last_assistant_found() {
        let msgs = vec![
            msg("assistant", "First"),
            msg("user", "Question"),
            msg("assistant", "Last answer"),
        ];
        assert_eq!(last_assistant_message(&msgs), Some("Last answer"));
    }

    #[test]
    fn last_assistant_none() {
        let msgs = vec![msg("user", "Only user")];
        assert_eq!(last_assistant_message(&msgs), None);
    }

    // -----------------------------------------------------------------------
    // is_code_path (private, tested via transcript_has_code_changes)
    // -----------------------------------------------------------------------

    #[test]
    fn code_changes_with_various_extensions() {
        for ext in &[".py", ".ts", ".tsx", ".js", ".go", ".java", ".cpp"] {
            let msgs = vec![msg_with_tools(
                "assistant",
                "",
                vec![ToolUse {
                    id: None,
                    name: "write".to_string(),
                    input: serde_json::json!({"path": format!("src/file{ext}")}),
                }],
                Vec::new(),
            )];
            assert!(
                transcript_has_code_changes(&msgs),
                "Expected code change for {ext}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // parse_transcript_message
    // -----------------------------------------------------------------------

    #[test]
    fn parse_simple_role_content() {
        let entry = serde_json::json!({
            "role": "user",
            "content": "Hello world"
        });
        let msg = parse_transcript_message(&entry).unwrap();
        assert_eq!(msg.role, "user");
        assert_eq!(msg.text, "Hello world");
    }

    #[test]
    fn parse_array_content_with_tools() {
        let entry = serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Thinking..."},
                {"type": "tool_use", "name": "bash", "id": "t1", "input": {"command": "ls"}}
            ]
        });
        let msg = parse_transcript_message(&entry).unwrap();
        assert_eq!(msg.text, "Thinking...");
        assert_eq!(msg.tool_uses.len(), 1);
        assert_eq!(msg.tool_uses[0].name, "bash");
    }

    #[test]
    fn parse_nested_message_format() {
        let entry = serde_json::json!({
            "message": {
                "role": "assistant",
                "content": "Nested content"
            }
        });
        let msg = parse_transcript_message(&entry).unwrap();
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.text, "Nested content");
    }
}
