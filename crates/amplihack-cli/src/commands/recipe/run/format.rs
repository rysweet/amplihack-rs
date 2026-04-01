use super::super::*;

const MAX_OUTPUT_LENGTH: usize = 200;

pub(super) fn format_recipe_run_result(
    result: &RecipeRunResult,
    format: OutputFormat,
    show_context: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => {
            let mut data = JsonMap::new();
            data.insert(
                "recipe_name".to_string(),
                JsonValue::String(result.recipe_name.clone()),
            );
            data.insert("success".to_string(), JsonValue::Bool(result.success));
            data.insert(
                "step_results".to_string(),
                JsonValue::Array(
                    result
                        .step_results
                        .iter()
                        .map(|step| {
                            json!({
                                "step_id": step.step_id,
                                "status": step.status,
                                "output": step.output,
                                "error": step.error,
                            })
                        })
                        .collect(),
                ),
            );
            if show_context && !result.context.is_empty() {
                data.insert(
                    "context".to_string(),
                    JsonValue::Object(result.context.clone()),
                );
            }
            Ok(serde_json::to_string_pretty(&JsonValue::Object(data))?)
        }
        OutputFormat::Yaml => {
            let mut data = JsonMap::new();
            data.insert(
                "recipe_name".to_string(),
                JsonValue::String(result.recipe_name.clone()),
            );
            data.insert("success".to_string(), JsonValue::Bool(result.success));
            data.insert(
                "step_results".to_string(),
                JsonValue::Array(
                    result
                        .step_results
                        .iter()
                        .map(|step| {
                            JsonValue::Object(JsonMap::from_iter([
                                (
                                    "step_id".to_string(),
                                    JsonValue::String(step.step_id.clone()),
                                ),
                                ("status".to_string(), JsonValue::String(step.status.clone())),
                                ("output".to_string(), JsonValue::String(step.output.clone())),
                                ("error".to_string(), JsonValue::String(step.error.clone())),
                            ]))
                        })
                        .collect(),
                ),
            );
            if show_context && !result.context.is_empty() {
                data.insert(
                    "context".to_string(),
                    JsonValue::Object(result.context.clone()),
                );
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
        lines.push(format!(
            "  {status_symbol} {}: {}",
            step.step_id, step.status
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

fn json_scalar_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(v) => v.to_string(),
        JsonValue::Number(v) => v.to_string(),
        JsonValue::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}
