use super::TranscriptMessage;
use super::analysis;
use std::fs;
use std::path::Path;

pub(super) fn collect_blockers(messages: &[TranscriptMessage], project_root: &Path) -> Vec<String> {
    let mut blockers = Vec::new();
    let has_code_changes = analysis::transcript_has_code_changes(messages);

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

    if has_code_changes
        && project_has_tests(project_root)
        && !analysis::has_successful_local_test(messages)
    {
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
        if analysis::normalize_tool_name(&tool_use.name) != "todowrite" {
            continue;
        }

        let items = analysis::extract_todo_items(&tool_use.input);
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

fn final_response_has_remaining_work(messages: &[TranscriptMessage]) -> bool {
    let Some(text) = analysis::last_assistant_message(messages) else {
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
    let Some(final_assistant) = analysis::last_assistant_message(messages) else {
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

    let Some(first_user) = analysis::first_user_message(messages) else {
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

pub(super) fn build_continuation_prompt(blockers: &[String]) -> String {
    let mut prompt = String::from("Continue working before stopping. Remaining items:\n");
    for blocker in blockers {
        prompt.push_str("- ");
        prompt.push_str(blocker);
        prompt.push('\n');
    }
    prompt.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::super::{ToolResult, ToolUse, TranscriptMessage};
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
    // build_continuation_prompt
    // -----------------------------------------------------------------------

    #[test]
    fn continuation_prompt_formats_blockers() {
        let blockers = vec!["Fix tests".to_string(), "Run linter".to_string()];
        let prompt = build_continuation_prompt(&blockers);
        assert!(prompt.contains("- Fix tests"));
        assert!(prompt.contains("- Run linter"));
        assert!(prompt.starts_with("Continue working"));
    }

    #[test]
    fn continuation_prompt_empty_blockers() {
        let prompt = build_continuation_prompt(&[]);
        assert!(prompt.contains("Remaining items:"));
    }

    // -----------------------------------------------------------------------
    // final_response_has_remaining_work
    // -----------------------------------------------------------------------

    #[test]
    fn remaining_work_detected() {
        let msgs = vec![
            msg("user", "Fix the bug"),
            msg(
                "assistant",
                "I fixed part of it. Next steps are to handle edge cases.",
            ),
        ];
        assert!(final_response_has_remaining_work(&msgs));
    }

    #[test]
    fn remaining_work_not_detected() {
        let msgs = vec![
            msg("user", "Fix the bug"),
            msg("assistant", "Done! The bug is fixed and all tests pass."),
        ];
        assert!(!final_response_has_remaining_work(&msgs));
    }

    #[test]
    fn remaining_work_no_assistant() {
        let msgs = vec![msg("user", "Fix the bug")];
        assert!(!final_response_has_remaining_work(&msgs));
    }

    // -----------------------------------------------------------------------
    // objective_appears_incomplete
    // -----------------------------------------------------------------------

    #[test]
    fn objective_incomplete_keywords() {
        let msgs = vec![
            msg("user", "Build the feature"),
            msg("assistant", "This is still incomplete — need more work."),
        ];
        assert!(objective_appears_incomplete(&msgs));
    }

    #[test]
    fn objective_complete() {
        let msgs = vec![
            msg("user", "Build the feature"),
            msg("assistant", "All done! Feature is implemented and tested."),
        ];
        assert!(!objective_appears_incomplete(&msgs));
    }

    #[test]
    fn objective_continue_request_without_done() {
        let msgs = vec![
            msg("user", "continue working on the API"),
            msg("assistant", "Working on it now."),
        ];
        assert!(objective_appears_incomplete(&msgs));
    }

    #[test]
    fn objective_continue_request_with_done() {
        let msgs = vec![
            msg("user", "continue working on the API"),
            msg("assistant", "All done. The API is complete and validated."),
        ];
        assert!(!objective_appears_incomplete(&msgs));
    }

    // -----------------------------------------------------------------------
    // incomplete_todos
    // -----------------------------------------------------------------------

    #[test]
    fn incomplete_todos_found() {
        let msgs = vec![msg_with_tools(
            "assistant",
            "",
            vec![ToolUse {
                id: None,
                name: "TodoWrite".to_string(),
                input: serde_json::json!({
                    "todos": [
                        {"content": "Write tests", "status": "pending"},
                        {"content": "Fix bug", "status": "completed"}
                    ]
                }),
            }],
            Vec::new(),
        )];
        let result = incomplete_todos(&msgs).unwrap();
        assert_eq!(result, vec!["Write tests"]);
    }

    #[test]
    fn all_todos_complete() {
        let msgs = vec![msg_with_tools(
            "assistant",
            "",
            vec![ToolUse {
                id: None,
                name: "TodoWrite".to_string(),
                input: serde_json::json!({
                    "todos": [
                        {"content": "Write tests", "status": "done"},
                        {"content": "Fix bug", "status": "completed"}
                    ]
                }),
            }],
            Vec::new(),
        )];
        let result = incomplete_todos(&msgs).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn no_todowrite_returns_none() {
        let msgs = vec![msg("assistant", "No tools")];
        assert!(incomplete_todos(&msgs).is_none());
    }

    // -----------------------------------------------------------------------
    // project_has_tests (uses temp dirs)
    // -----------------------------------------------------------------------

    #[test]
    fn project_has_tests_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tests")).unwrap();
        assert!(project_has_tests(dir.path()));
    }

    #[test]
    fn project_has_test_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test_main.py"), "").unwrap();
        assert!(project_has_tests(dir.path()));
    }

    #[test]
    fn project_no_tests() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        assert!(!project_has_tests(dir.path()));
    }
}
