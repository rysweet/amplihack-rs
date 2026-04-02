use super::*;

pub(crate) fn parse_recipe_from_input(
    input: &str,
    working_dir: impl AsRef<Path>,
) -> Result<RecipeDoc> {
    parse_recipe_from_path(resolve_recipe_path(input, working_dir)?)
}

pub(crate) fn parse_recipe_from_path(path: impl AsRef<Path>) -> Result<RecipeDoc> {
    let validated = validate_path(path.as_ref(), false)?;
    let text = fs::read_to_string(&validated)
        .with_context(|| format!("Recipe file not found: {}", validated.display()))?;

    if text.len() > MAX_YAML_SIZE_BYTES {
        anyhow::bail!(
            "Recipe file too large ({} bytes). Maximum allowed: {} bytes",
            text.len(),
            MAX_YAML_SIZE_BYTES
        );
    }

    parse_recipe_text(&text)
}

pub(crate) fn parse_recipe_text(text: &str) -> Result<RecipeDoc> {
    if text.len() > MAX_YAML_SIZE_BYTES {
        anyhow::bail!(
            "YAML content too large ({} bytes). Maximum allowed: {} bytes",
            text.len(),
            MAX_YAML_SIZE_BYTES
        );
    }

    let raw_value: Value = serde_yaml::from_str(text)?;
    let raw_mapping = raw_value
        .as_mapping()
        .context("Recipe YAML must be a mapping at the top level")?;

    require_field(raw_mapping, "name", "Recipe must have a 'name' field")?;

    if let Some(name_val) = raw_mapping.get(Value::String("name".to_string()))
        && name_val.is_null()
    {
        anyhow::bail!("Recipe must have a 'name' field");
    }

    require_field(
        raw_mapping,
        "steps",
        "Recipe must have a 'steps' field with at least one step",
    )?;

    if let Some(steps_val) = raw_mapping.get(Value::String("steps".to_string()))
        && let Some(steps_seq) = steps_val.as_sequence()
    {
        for step_val in steps_seq {
            if let Some(step_map) = step_val.as_mapping() {
                let id_key = Value::String("id".to_string());
                let should_bail = match step_map.get(&id_key) {
                    None => true,
                    Some(v) if v.is_null() => true,
                    _ => false,
                };
                if should_bail {
                    anyhow::bail!("Every step must have a non-empty 'id' field");
                }
            }
        }
    }

    let recipe: RecipeDoc = serde_yaml::from_value(raw_value)?;
    if recipe.steps.is_empty() {
        anyhow::bail!("Recipe must have a 'steps' field with at least one step");
    }

    let mut seen_ids = BTreeSet::new();
    for step in &recipe.steps {
        if step.id.trim().is_empty() {
            anyhow::bail!("Every step must have a non-empty 'id' field");
        }
        if !seen_ids.insert(step.id.clone()) {
            anyhow::bail!("Duplicate step id: '{}'", step.id);
        }
        if let Some(step_type) = &step.step_type {
            match step_type.to_ascii_lowercase().as_str() {
                "bash" | "agent" | "recipe" => {}
                other => anyhow::bail!("'{}' is not a valid StepType", other),
            }
        }
    }

    Ok(recipe)
}

pub(crate) fn require_field(mapping: &serde_yaml::Mapping, key: &str, message: &str) -> Result<()> {
    if mapping.contains_key(Value::String(key.to_string())) {
        return Ok(());
    }
    anyhow::bail!(message.to_string())
}

pub(crate) fn validate_path(path: impl AsRef<Path>, must_exist: bool) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let resolved = super::resolve::resolve_path_from(&cwd, path)?;

    if must_exist && !resolved.exists() {
        anyhow::bail!("Path does not exist: {}", resolved.display());
    }

    Ok(resolved)
}
