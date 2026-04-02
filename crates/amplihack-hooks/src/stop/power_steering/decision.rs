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
