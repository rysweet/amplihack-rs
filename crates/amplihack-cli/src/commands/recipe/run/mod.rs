use super::*;

mod binary;
mod execute;
mod format;

use execute::execute_recipe_via_rust;
use format::format_recipe_run_result;

pub fn run_recipe(
    recipe_path: &str,
    context_args: &[String],
    dry_run: bool,
    verbose: bool,
    format: &str,
    working_dir: Option<&str>,
    step_timeout: Option<u64>,
) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let (context, errors) = parse_context_args(context_args);
    if !errors.is_empty() {
        for error in errors {
            writeln!(io::stderr(), "Error: {error}")?;
        }
        return Err(exit_error(1));
    }

    let working_dir = working_dir.unwrap_or(".");
    let abs_working_dir = validate_path(working_dir, false)?;
    let validated_path = resolve_recipe_path(recipe_path, &abs_working_dir)?;
    let recipe = parse_recipe_from_path(&validated_path)?;
    let (merged_context, inferred) = infer_missing_context(&recipe.context, &context);
    if verbose {
        writeln!(io::stderr(), "Executing recipe: {}", recipe.name)?;
        if dry_run {
            writeln!(io::stderr(), "DRY RUN MODE - No actual execution")?;
        }
        if !inferred.is_empty() {
            writeln!(
                io::stderr(),
                "[context] Inferred {} variable(s): {}",
                inferred.len(),
                inferred.join(", ")
            )?;
        }
    }
    let result = match execute_recipe_via_rust(
        &validated_path,
        &merged_context,
        dry_run,
        verbose,
        &abs_working_dir,
        step_timeout,
    ) {
        Ok(result) => result,
        Err(error) => {
            writeln!(io::stderr(), "Error: {error}")?;
            return Err(exit_error(1));
        }
    };

    println!("{}", format_recipe_run_result(&result, format, false)?);

    if result.success {
        Ok(())
    } else {
        Err(exit_error(1))
    }
}

fn parse_context_args(context_args: &[String]) -> (BTreeMap<String, String>, Vec<String>) {
    let mut context = BTreeMap::new();
    let mut errors = Vec::new();

    for arg in context_args {
        if let Some((key, value)) = arg.split_once('=') {
            context.insert(key.to_string(), value.to_string());
        } else {
            errors.push(format!(
                "Invalid context format '{arg}'. Use key=value format (e.g., -c 'question=What is X?' -c 'var=value')"
            ));
        }
    }

    (context, errors)
}

fn infer_missing_context(
    recipe_defaults: &BTreeMap<String, Value>,
    user_context: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut merged = recipe_defaults
        .iter()
        .map(|(key, value)| (key.clone(), scalar_to_context_value(value)))
        .collect::<BTreeMap<_, _>>();

    for (key, value) in user_context {
        merged.insert(key.clone(), value.clone());
    }

    let mut inferred = Vec::new();
    let keys = merged.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if merged.get(&key).is_some_and(|value| !value.is_empty()) {
            continue;
        }

        let env_key = format!("AMPLIHACK_CONTEXT_{}", key.to_uppercase());
        if let Ok(value) = std::env::var(&env_key)
            && !value.is_empty()
        {
            merged.insert(key.clone(), value);
            inferred.push(format!("{key} (from ${env_key})"));
            continue;
        }

        if key == "task_description"
            && let Ok(value) = std::env::var("AMPLIHACK_TASK_DESCRIPTION")
            && !value.is_empty()
        {
            merged.insert(key.clone(), value);
            inferred.push(format!("{key} (from $AMPLIHACK_TASK_DESCRIPTION)"));
        } else if key == "repo_path" {
            let value = std::env::var("AMPLIHACK_REPO_PATH").unwrap_or_else(|_| ".".to_string());
            if value != "." {
                inferred.push(format!("{key} (from $AMPLIHACK_REPO_PATH)"));
            }
            merged.insert(key.clone(), value);
        }
    }

    (merged, inferred)
}

fn scalar_to_context_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(v) => {
            if *v {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

#[cfg(test)]
mod tests_context;
#[cfg(test)]
mod tests_execute;
#[cfg(test)]
mod tests_format;
