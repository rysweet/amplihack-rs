//! Frame rendering helpers for auto-mode UI.

use crate::auto_mode_state::AutoModeState;

const MAX_TITLE_LEN: usize = 50;
const MAX_LOG_LINES: usize = 50;

pub fn generate_title_from_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return "Auto Mode Session".to_string();
    }
    if trimmed.chars().count() <= MAX_TITLE_LEN {
        return trimmed.to_string();
    }
    let mut title = trimmed.chars().take(MAX_TITLE_LEN - 3).collect::<String>();
    title.push_str("...");
    title
}

pub fn render_auto_mode_frame(
    state: &AutoModeState,
    prompt: &str,
    showing_help: bool,
    queued_inputs: usize,
) -> String {
    let snapshot = state.snapshot();
    let title = generate_title_from_prompt(prompt);
    let elapsed = format_elapsed(snapshot.start_time);
    let status = format_status(&snapshot.status);
    let input_tokens = snapshot.costs.input_tokens;
    let output_tokens = snapshot.costs.output_tokens;
    let estimated_cost = snapshot.costs.estimated_cost;

    let mut lines = vec![
        format!("=== {} ===", title),
        format!(
            "Turn: {}/{} | Time: {} | Status: {}",
            snapshot.turn, snapshot.max_turns, elapsed, status
        ),
        format!(
            "Input: {} | Output: {} | Cost: ${:.4}",
            input_tokens, output_tokens, estimated_cost
        ),
        String::new(),
        "[Tasks]".to_string(),
    ];

    if snapshot.todos.is_empty() {
        lines.push("  No tasks yet".to_string());
    } else {
        for todo in &snapshot.todos {
            let status = todo.get("status").map(String::as_str).unwrap_or("pending");
            let content = todo
                .get("content")
                .or_else(|| todo.get("title"))
                .map(String::as_str)
                .unwrap_or("");
            lines.push(format!("  {} {}", todo_icon(status), content));
        }
    }

    lines.push(String::new());
    lines.push("[Logs]".to_string());
    if snapshot.logs.is_empty() {
        lines.push("  Waiting for logs...".to_string());
    } else {
        let start = snapshot.logs.len().saturating_sub(MAX_LOG_LINES);
        for log in &snapshot.logs[start..] {
            lines.push(format!("  {log}"));
        }
    }

    lines.push(String::new());
    lines.push("[Controls]".to_string());
    lines.push("  x = exit UI (auto mode continues)".to_string());
    lines.push("  h = toggle help".to_string());
    if queued_inputs > 0 {
        lines.push(format!("  queued instructions: {queued_inputs}"));
    }
    if showing_help {
        lines.push(String::new());
        lines.push("[Help]".to_string());
        lines.push("  Auto mode keeps running after UI exit.".to_string());
        lines.push("  Use --append to inject new instructions from another shell.".to_string());
    }

    lines.join("\n")
}

fn format_status(status: &str) -> String {
    match status {
        "running" => "▶ RUNNING".to_string(),
        "completed" => "✓ COMPLETED".to_string(),
        "error" => "✗ ERROR".to_string(),
        other => format!("◆ {}", other.to_ascii_uppercase()),
    }
}

fn todo_icon(status: &str) -> &'static str {
    match status {
        "completed" => "✓",
        "in_progress" => "▶",
        _ => "⏸",
    }
}

fn format_elapsed(start_time: f64) -> String {
    if start_time <= 0.0 {
        return "0s".to_string();
    }
    let elapsed = (chrono::Utc::now().timestamp_millis() as f64 / 1000.0 - start_time).max(0.0);
    if elapsed < 60.0 {
        format!("{}s", elapsed as u64)
    } else {
        format!("{}m {}s", (elapsed as u64) / 60, (elapsed as u64) % 60)
    }
}
