//! Power steering: checks whether the agent has finished enough work to stop.
//!
//! The checker is intentionally fail-open: if transcript analysis fails, the stop
//! hook approves instead of trapping the user. Within that boundary it now runs
//! native Rust analysis instead of delegating to a stale Python bridge.

use amplihack_state::AtomicCounter;
use amplihack_types::{ProjectDirs, sanitize_session_id};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
struct PowerSteeringConfig {
    enabled: bool,
}

impl Default for PowerSteeringConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TranscriptMessage {
    role: String,
    text: String,
    tool_uses: Vec<ToolUse>,
    tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolUse {
    id: Option<String>,
    name: String,
    input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolResult {
    tool_use_id: Option<String>,
    is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TodoItem {
    label: String,
    status: String,
}

/// Check if power steering should run for this project.
pub fn should_run(dirs: &ProjectDirs) -> bool {
    load_config(dirs)
        .map(|config| config.enabled)
        .unwrap_or(false)
}

/// Check power steering state and decide whether to block.
///
/// Returns `Some(block_json)` if the session should be blocked,
/// `None` if it should be approved.
pub fn check(
    dirs: &ProjectDirs,
    session_id: &str,
    transcript_path: Option<&Path>,
) -> anyhow::Result<Option<Value>> {
    let power_steering_dir = dirs.session_power_steering(session_id);
    fs::create_dir_all(&power_steering_dir)?;
    fs::create_dir_all(&dirs.power_steering)?;

    let counter = AtomicCounter::new(power_steering_dir.join("session_count"));
    let count = counter.increment()?;

    // First stop: let the agent end naturally, only enforce on repeated stop
    // attempts after it decided to continue working.
    if count <= 1 {
        return Ok(None);
    }

    if is_disabled(dirs) || already_completed(dirs, session_id) {
        return Ok(None);
    }

    let Some(path) = transcript_path else {
        tracing::warn!("Power steering transcript missing, approving");
        return Ok(None);
    };

    let messages = match read_transcript_messages(path) {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!("Power steering transcript parsing failed, approving: {error}");
            return Ok(None);
        }
    };

    if messages.is_empty() || is_qa_session(&messages) {
        return Ok(None);
    }

    let blockers = collect_blockers(&messages, &dirs.root);
    if blockers.is_empty() {
        mark_complete(dirs, session_id)?;
        write_summary(dirs, session_id, &messages)?;
        return Ok(None);
    }

    Ok(Some(serde_json::json!({
        "decision": "block",
        "reason": build_continuation_prompt(&blockers),
    })))
}

fn load_config(dirs: &ProjectDirs) -> Option<PowerSteeringConfig> {
    let path = dirs.power_steering_config();
    if !path.exists() {
        return None;
    }

    match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => Some(PowerSteeringConfig {
                enabled: value
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            }),
            Err(error) => {
                tracing::warn!(
                    "Invalid power steering config at {}: {error}",
                    path.display()
                );
                Some(PowerSteeringConfig::default())
            }
        },
        Err(error) => {
            tracing::warn!(
                "Failed reading power steering config at {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn is_disabled(dirs: &ProjectDirs) -> bool {
    if std::env::var_os("AMPLIHACK_SKIP_POWER_STEERING").is_some() {
        return true;
    }

    if dirs.power_steering.join(".disabled").exists() {
        return true;
    }

    load_config(dirs)
        .map(|config| !config.enabled)
        .unwrap_or(false)
}

fn already_completed(dirs: &ProjectDirs, session_id: &str) -> bool {
    completion_semaphore(dirs, session_id).exists()
}

fn mark_complete(dirs: &ProjectDirs, session_id: &str) -> anyhow::Result<()> {
    let semaphore = completion_semaphore(dirs, session_id);
    if let Some(parent) = semaphore.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(semaphore, "")?;
    Ok(())
}

fn completion_semaphore(dirs: &ProjectDirs, session_id: &str) -> PathBuf {
    dirs.power_steering
        .join(format!(".{}_completed", sanitize_session_id(session_id)))
}

fn write_summary(
    dirs: &ProjectDirs,
    session_id: &str,
    messages: &[TranscriptMessage],
) -> anyhow::Result<()> {
    let session_dir = dirs.session_power_steering(session_id);
    fs::create_dir_all(&session_dir)?;

    let first_user = first_user_message(messages).unwrap_or("unknown task");
    let final_assistant =
        last_assistant_message(messages).unwrap_or("no assistant summary recorded");
    let summary = format!(
        "# Power Steering Summary\n\n\
         - Session ID: `{session_id}`\n\
         - Status: approved\n\
         - First user request: {}\n\
         - Final assistant summary: {}\n",
        first_user.trim(),
        final_assistant.trim()
    );

    fs::write(session_dir.join("summary.md"), summary)?;
    Ok(())
}

fn read_transcript_messages(path: &Path) -> anyhow::Result<Vec<TranscriptMessage>> {
    let raw = fs::read_to_string(path)?;
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

fn is_qa_session(messages: &[TranscriptMessage]) -> bool {
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

fn collect_blockers(messages: &[TranscriptMessage], project_root: &Path) -> Vec<String> {
    let mut blockers = Vec::new();
    let has_code_changes = transcript_has_code_changes(messages);

    if let Some(incomplete_todos) = incomplete_todos(messages)
        && !incomplete_todos.is_empty()
    {
        blockers.push(format!(
            "Complete all tracked TodoWrite items before stopping: {}.",
            incomplete_todos.join(", ")
        ));
    }

    if final_response_has_remaining_work(messages) {
        blockers.push(
            "The final response still describes remaining work or future agent actions."
                .to_string(),
        );
    }

    if objective_appears_incomplete(messages) {
        blockers.push(
            "The final response does not clearly indicate that the user's request is complete."
                .to_string(),
        );
    }

    if has_code_changes && project_has_tests(project_root) && !has_successful_local_test(messages) {
        blockers
            .push("Run local validation/tests and confirm they pass before stopping.".to_string());
    }

    blockers
}

fn incomplete_todos(messages: &[TranscriptMessage]) -> Option<Vec<String>> {
    for tool_use in messages
        .iter()
        .flat_map(|message| message.tool_uses.iter())
        .rev()
    {
        if normalize_tool_name(&tool_use.name) != "todowrite" {
            continue;
        }

        let items = extract_todo_items(&tool_use.input);
        if items.is_empty() {
            return Some(Vec::new());
        }

        let incomplete = items
            .into_iter()
            .filter(|item| !matches!(item.status.as_str(), "completed" | "complete" | "done"))
            .map(|item| item.label)
            .collect::<Vec<_>>();
        return Some(incomplete);
    }

    None
}

fn extract_todo_items(input: &Value) -> Vec<TodoItem> {
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

fn final_response_has_remaining_work(messages: &[TranscriptMessage]) -> bool {
    let Some(text) = last_assistant_message(messages) else {
        return false;
    };

    let lowered = text.to_ascii_lowercase();
    [
        "next steps",
        "still need",
        "remaining work",
        "follow-up",
        "future work",
        "will address",
        "left to do",
        "left undone",
        "todo:",
        "todo ",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

fn objective_appears_incomplete(messages: &[TranscriptMessage]) -> bool {
    let Some(final_assistant) = last_assistant_message(messages) else {
        return false;
    };

    let lowered = final_assistant.to_ascii_lowercase();
    if [
        "not finished",
        "incomplete",
        "partial",
        "remaining work",
        "need to continue",
        "still need",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        return true;
    }

    let Some(first_user) = first_user_message(messages) else {
        return false;
    };
    let first_user_lowered = first_user.to_ascii_lowercase();
    if first_user_lowered.contains("continue") {
        return ![
            "done",
            "completed",
            "implemented",
            "fixed",
            "updated",
            "validated",
            "passed",
            "all done",
            "no remaining work",
            "finished",
        ]
        .iter()
        .any(|needle| lowered.contains(needle));
    }

    false
}

fn transcript_has_code_changes(messages: &[TranscriptMessage]) -> bool {
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

fn has_successful_local_test(messages: &[TranscriptMessage]) -> bool {
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
        "python -m pytest",
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

fn project_has_tests(root: &Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
    let mut visited_dirs = 0usize;

    while let Some(dir) = stack.pop() {
        if visited_dirs > 200 {
            break;
        }
        visited_dirs += 1;

        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };

            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if matches!(
                    name.as_ref(),
                    ".git" | "target" | "node_modules" | ".venv" | "venv"
                ) {
                    continue;
                }
                if matches!(name.as_ref(), "tests" | "test" | "__tests__") {
                    return true;
                }
                stack.push(path);
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("test_")
                || name.ends_with("_test.rs")
                || name.ends_with("_test.py")
                || name.ends_with(".test.js")
                || name.ends_with(".test.ts")
                || name.ends_with(".spec.js")
                || name.ends_with(".spec.ts")
            {
                return true;
            }
        }
    }

    false
}

fn build_continuation_prompt(blockers: &[String]) -> String {
    let mut prompt = String::from("Continue working before stopping. Remaining items:\n");
    for blocker in blockers {
        prompt.push_str("- ");
        prompt.push_str(blocker);
        prompt.push('\n');
    }
    prompt.trim_end().to_string()
}

fn first_user_message(messages: &[TranscriptMessage]) -> Option<&str> {
    messages
        .iter()
        .find(|message| message.role == "user" && !message.text.is_empty())
        .map(|message| message.text.as_str())
}

fn last_assistant_message(messages: &[TranscriptMessage]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.text.is_empty())
        .map(|message| message.text.as_str())
}

fn normalize_tool_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_enabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(!should_run(&dirs));
    }

    #[test]
    fn enabled_when_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(dirs.power_steering_config(), r#"{"enabled": true}"#).unwrap();
        assert!(should_run(&dirs));
    }

    #[test]
    fn first_stop_always_approves() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement feature"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Next steps: run tests"}]}}"#,
        );

        let result = check(&dirs, "session-1", Some(&transcript)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn blocks_when_todos_remain_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Finish the migration"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"todo_1","name":"TodoWrite","input":{"todos":[{"content":"Port power steering","status":"in_progress"},{"content":"Run tests","status":"pending"}]}}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done for now."}]}}"#,
        );

        assert!(
            check(&dirs, "session-2", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-2", Some(&transcript))
            .unwrap()
            .unwrap();
        let reason = result.get("reason").and_then(Value::as_str).unwrap();
        assert!(reason.contains("TodoWrite"));
        assert_eq!(result["decision"], "block");
    }

    #[test]
    fn blocks_when_final_response_lists_next_steps() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Continue fixing the Rust port"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit_1","name":"Edit","input":{"file_path":"src/lib.rs"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit_1","content":"ok","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented the change. Next steps: run cargo test and finish the remaining cleanup."}]}}"#,
        );

        assert!(
            check(&dirs, "session-3", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-3", Some(&transcript))
            .unwrap()
            .unwrap();
        let reason = result.get("reason").and_then(Value::as_str).unwrap();
        assert!(reason.contains("remaining work"));
    }

    #[test]
    fn blocks_when_code_changed_without_tests() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement the fix"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit_1","name":"Edit","input":{"file_path":"src/main.rs"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit_1","content":"ok","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented the fix and updated the code."}]}}"#,
        );

        assert!(
            check(&dirs, "session-4", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-4", Some(&transcript))
            .unwrap()
            .unwrap();
        let reason = result.get("reason").and_then(Value::as_str).unwrap();
        assert!(reason.contains("local validation/tests"));
    }

    #[test]
    fn approves_and_marks_complete_after_successful_test_run() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement the fix"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit_1","name":"Edit","input":{"file_path":"src/main.rs"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit_1","content":"ok","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"bash_1","name":"Bash","input":{"command":"cargo test -p amplihack-hooks power_steering -- --nocapture"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash_1","content":"test result: ok. 6 passed; 0 failed","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented the fix, ran cargo test, and all tests passed."}]}}"#,
        );

        assert!(
            check(&dirs, "session-5", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-5", Some(&transcript)).unwrap();
        assert!(result.is_none());
        assert!(completion_semaphore(&dirs, "session-5").exists());
        assert!(
            dirs.session_power_steering("session-5")
                .join("summary.md")
                .exists()
        );
    }

    #[test]
    fn qa_session_skips_power_steering() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"How do I run the tests?"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Use cargo test from the repo root."}]}}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"What about a single package?"}]}}"#,
        );

        assert!(
            check(&dirs, "session-6", Some(&transcript))
                .unwrap()
                .is_none()
        );
        assert!(
            check(&dirs, "session-6", Some(&transcript))
                .unwrap()
                .is_none()
        );
    }

    fn configured_dirs(root: &Path) -> ProjectDirs {
        let dirs = ProjectDirs::new(root);
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(dirs.power_steering_config(), r#"{"enabled": true}"#).unwrap();
        dirs
    }

    fn write_transcript(root: &Path, contents: &str) -> PathBuf {
        let path = root.join("transcript.jsonl");
        fs::write(&path, contents).unwrap();
        path
    }
}
