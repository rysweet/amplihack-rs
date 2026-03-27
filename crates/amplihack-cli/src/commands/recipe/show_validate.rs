use super::*;

pub fn run_validate(file: &str, verbose: bool, format: &str) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let mut stdout = io::stdout();

    match parse_recipe_from_input(file, ".") {
        Ok(recipe) => {
            writeln!(
                stdout,
                "{}",
                format_validation_result(Some(&recipe), true, &[], format, verbose)?
            )?;
            Ok(())
        }
        Err(error) => {
            writeln!(
                stdout,
                "{}",
                format_validation_result(None, false, &[error.to_string()], format, verbose)?
            )?;
            Err(exit_error(1))
        }
    }
}

pub fn run_show(name: &str, format: &str, show_steps: bool, show_context: bool) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let mut stdout = io::stdout();

    match parse_recipe_from_input(name, ".") {
        Ok(recipe) => {
            writeln!(
                stdout,
                "{}",
                format_recipe_details(&recipe, format, show_steps, show_context)?
            )?;
            Ok(())
        }
        Err(error) => {
            writeln!(io::stderr(), "Error: {error}")?;
            Err(exit_error(1))
        }
    }
}

pub(crate) fn format_validation_result(
    recipe: Option<&RecipeDoc>,
    is_valid: bool,
    errors: &[String],
    format: OutputFormat,
    verbose: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => {
            let mut data = serde_json::Map::new();
            data.insert("valid".into(), json!(is_valid));
            data.insert("errors".into(), json!(errors));
            if let Some(recipe) = recipe {
                data.insert("recipe_name".into(), json!(recipe.name));
            }
            Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
                data,
            ))?)
        }
        OutputFormat::Yaml => {
            let mut data = Mapping::new();
            data.insert(
                Value::String("valid".into()),
                serde_yaml::to_value(is_valid)?,
            );
            data.insert(
                Value::String("errors".into()),
                serde_yaml::to_value(errors)?,
            );
            if let Some(recipe) = recipe {
                data.insert(
                    Value::String("recipe_name".into()),
                    Value::String(recipe.name.clone()),
                );
            }
            Ok(serde_yaml::to_string(&Value::Mapping(data))?)
        }
        OutputFormat::Table => {
            let mut lines = Vec::new();
            if is_valid {
                lines.push("✓ Recipe is valid".to_string());
                if let Some(recipe) = recipe {
                    lines.push(format!("  Name: {}", recipe.name));
                    if verbose {
                        lines.push(format!(
                            "  Description: {}",
                            if recipe.description.is_empty() {
                                "(none)"
                            } else {
                                &recipe.description
                            }
                        ));
                        lines.push(format!("  Steps: {}", recipe.steps.len()));
                    }
                }
            } else {
                lines.push("✗ Recipe is invalid".to_string());
                if !errors.is_empty() {
                    lines.push(String::new());
                    lines.push("Errors:".to_string());
                    for error in errors {
                        lines.push(format!("  • {}", error));
                    }
                }
            }
            Ok(lines.join("\n"))
        }
    }
}

pub(crate) fn format_recipe_details(
    recipe: &RecipeDoc,
    format: OutputFormat,
    show_steps: bool,
    show_context: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => Ok(serde_json::to_string_pretty(&json!({
            "name": recipe.name,
            "description": recipe.description,
            "version": recipe.version,
            "author": recipe.author,
            "tags": recipe.tags,
            "steps": recipe.steps.iter().map(step_json).collect::<Vec<_>>(),
            "context": recipe.context,
        }))?),
        OutputFormat::Yaml => {
            let mut root = Mapping::new();
            root.insert(
                Value::String("name".into()),
                Value::String(recipe.name.clone()),
            );
            root.insert(
                Value::String("description".into()),
                Value::String(recipe.description.clone()),
            );
            root.insert(
                Value::String("version".into()),
                Value::String(recipe.version.clone()),
            );
            root.insert(
                Value::String("author".into()),
                Value::String(recipe.author.clone()),
            );
            root.insert(
                Value::String("tags".into()),
                serde_yaml::to_value(&recipe.tags)?,
            );
            root.insert(
                Value::String("steps".into()),
                serde_yaml::to_value(&recipe.steps)?,
            );
            root.insert(
                Value::String("context".into()),
                serde_yaml::to_value(&recipe.context)?,
            );
            Ok(serde_yaml::to_string(&Value::Mapping(root))?)
        }
        OutputFormat::Table => {
            let mut lines = vec![
                format!("Recipe: {}", recipe.name),
                format!(
                    "Description: {}",
                    if recipe.description.is_empty() {
                        "(none)"
                    } else {
                        &recipe.description
                    }
                ),
                format!(
                    "Version: {}",
                    if recipe.version.is_empty() {
                        "(not specified)"
                    } else {
                        &recipe.version
                    }
                ),
                format!(
                    "Author: {}",
                    if recipe.author.is_empty() {
                        "(not specified)"
                    } else {
                        &recipe.author
                    }
                ),
            ];

            if !recipe.tags.is_empty() {
                lines.push(format!("Tags: {}", recipe.tags.join(", ")));
            }

            if show_steps && !recipe.steps.is_empty() {
                lines.push(String::new());
                lines.push(format!("Steps ({}):", recipe.steps.len()));
                for (index, step) in recipe.steps.iter().enumerate() {
                    lines.push(format!(
                        "  {}. {} ({})",
                        index + 1,
                        step.id,
                        infer_step_type(step)
                    ));
                    if let Some(command) = &step.command {
                        lines.push(format!("     Command: {}", command));
                    }
                    if let Some(agent) = &step.agent {
                        lines.push(format!("     Agent: {}", agent));
                    }
                    if let Some(prompt) = &step.prompt {
                        let prompt = if prompt.len() > 100 {
                            format!("{}...", &prompt[..100])
                        } else {
                            prompt.clone()
                        };
                        lines.push(format!("     Prompt: {}", prompt));
                    }
                }
            }

            if show_context && !recipe.context.is_empty() {
                lines.push(String::new());
                lines.push("Context Variables:".to_string());
                for (key, value) in &recipe.context {
                    lines.push(format!("  {}: {}", key, yaml_scalar(value)));
                }
            }

            Ok(lines.join("\n"))
        }
    }
}

fn step_json(step: &RawStep) -> serde_json::Value {
    json!({
        "id": step.id,
        "type": infer_step_type(step),
        "command": step.command,
        "agent": step.agent,
        "prompt": step.prompt,
    })
}

fn infer_step_type(step: &RawStep) -> &'static str {
    match step.step_type.as_deref() {
        Some("bash") | Some("BASH") => "bash",
        Some("agent") | Some("AGENT") => "agent",
        Some("recipe") | Some("RECIPE") => "recipe",
        Some(_) => "bash",
        None if step.recipe.is_some() => "recipe",
        None if step.agent.is_some() => "agent",
        None if step.prompt.is_some() && step.command.is_none() => "agent",
        _ => "bash",
    }
}

fn yaml_scalar(value: &Value) -> String {
    match value {
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}
