use super::super::*;

const MAX_OUTPUT_LENGTH: usize = 200;
const MAX_RECENT_SNIPPET_LINES: usize = 5;
const MAX_RECENT_SNIPPET_CHARS: usize = 300;

pub(super) fn format_recipe_run_result(
    result: &RecipeRunResult,
    format: OutputFormat,
    show_context: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => {
            let mut data = serde_json::to_value(result)?
                .as_object()
                .cloned()
                .unwrap_or_default();
            if !show_context || result.context.is_empty() {
                data.remove("context");
            }
            Ok(serde_json::to_string_pretty(&JsonValue::Object(data))?)
        }
        OutputFormat::Yaml => {
            let mut data = serde_json::to_value(result)?
                .as_object()
                .cloned()
                .unwrap_or_default();
            if !show_context || result.context.is_empty() {
                data.remove("context");
            }
            Ok(serde_yaml::to_string(&JsonValue::Object(data))?)
        }
        OutputFormat::Table => Ok(format_recipe_run_table(result, show_context)),
    }
}

fn format_recipe_run_table(result: &RecipeRunResult, show_context: bool) -> String {
    let mut lines = vec![
        format!("Recipe: {}", result.recipe_name),
        format!(
            "Status: {}",
            if result.success {
                "✓ Success"
            } else {
                "✗ Failed"
            }
        ),
        String::new(),
    ];

    if result.step_results.is_empty() {
        lines.push("No steps executed (0 steps)".to_string());
        return lines.join("\n");
    }

    lines.push("Steps:".to_string());
    for step in &result.step_results {
        let status_symbol = match step.status.as_str() {
            "completed" => "✓",
            "failed" => "✗",
            "skipped" => "⊘",
            _ => "?",
        };
        let step_label = match &step.step_name {
            Some(name) if !name.is_empty() => format!("{} ({name})", step.step_id),
            _ => step.step_id.clone(),
        };
        let mut details = Vec::new();
        if let Some(phase) = &step.phase
            && !phase.is_empty()
        {
            details.push(format!("phase: {phase}"));
        }
        if let Some(elapsed_ms) = step.elapsed_ms {
            details.push(format!("elapsed: {}", format_elapsed(elapsed_ms)));
        }
        if let Some(child) = &step.child
            && let Some(label) = format_child(child)
        {
            details.push(format!("child: {label}"));
        }
        let detail_suffix = if details.is_empty() {
            String::new()
        } else {
            format!(" [{}]", details.join(", "))
        };
        lines.push(format!(
            "  {status_symbol} {step_label}: {}{detail_suffix}",
            step.status
        ));

        if !step.output.is_empty() {
            let output = if step.output.chars().count() > MAX_OUTPUT_LENGTH {
                tracing::warn!(
                    step_id = %step.step_id,
                    original_len = step.output.chars().count(),
                    max_len = MAX_OUTPUT_LENGTH,
                    "Step output truncated"
                );
                format!(
                    "{}... (truncated)",
                    step.output
                        .chars()
                        .take(MAX_OUTPUT_LENGTH)
                        .collect::<String>()
                )
            } else {
                step.output.clone()
            };
            lines.push(format!("    Output: {output}"));
        }

        if !step.error.is_empty() {
            lines.push(format!("    Error: {}", step.error));
        }
        append_recent_snippet_lines(&mut lines, "Recent stdout", &step.recent_stdout);
        append_recent_snippet_lines(&mut lines, "Recent stderr", &step.recent_stderr);
    }

    if show_context && !result.context.is_empty() {
        lines.push(String::new());
        lines.push("Context:".to_string());
        for (key, value) in &result.context {
            lines.push(format!("  {key}: {}", json_scalar_to_string(value)));
        }
    }

    lines.join("\n")
}

fn format_elapsed(elapsed_ms: u64) -> String {
    if elapsed_ms >= 1_000 && elapsed_ms.is_multiple_of(1_000) {
        format!("{}s", elapsed_ms / 1_000)
    } else if elapsed_ms >= 1_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        format!("{elapsed_ms}ms")
    }
}

fn format_child(child: &JsonValue) -> Option<String> {
    match child {
        JsonValue::String(value) if !value.is_empty() => Some(value.clone()),
        JsonValue::Object(map) => {
            let kind = map.get("kind").and_then(JsonValue::as_str).unwrap_or("");
            let name = map.get("name").and_then(JsonValue::as_str).unwrap_or("");
            match (kind.is_empty(), name.is_empty()) {
                (false, false) => Some(format!("{kind} {name}")),
                (false, true) => Some(kind.to_string()),
                (true, false) => Some(name.to_string()),
                (true, true) => None,
            }
        }
        _ => None,
    }
}

fn append_recent_snippet_lines(lines: &mut Vec<String>, label: &str, snippets: &[String]) {
    if snippets.is_empty() {
        return;
    }

    lines.push(format!("    {label}:"));
    let start = snippets.len().saturating_sub(MAX_RECENT_SNIPPET_LINES);
    for snippet in &snippets[start..] {
        lines.push(format!("      {}", truncate_snippet(snippet)));
    }
}

fn truncate_snippet(value: &str) -> String {
    if value.chars().count() <= MAX_RECENT_SNIPPET_CHARS {
        return value.to_string();
    }
    format!(
        "{}... (truncated)",
        value
            .chars()
            .take(MAX_RECENT_SNIPPET_CHARS)
            .collect::<String>()
    )
}

fn json_scalar_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(v) => v.to_string(),
        JsonValue::Number(v) => v.to_string(),
        JsonValue::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}
