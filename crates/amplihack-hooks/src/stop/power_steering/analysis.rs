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
