use super::*;

pub fn run_list(
    recipe_dir: Option<&str>,
    format: &str,
    tags: &[String],
    verbose: bool,
) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let search_dirs = recipe_search_dirs(recipe_dir, ".")?;
    let recipes = discover_recipes(&search_dirs)?;
    let filtered = filter_by_tags(recipes, tags);
    println!("{}", format_recipe_list(&filtered, format, verbose)?);
    Ok(())
}

pub(crate) fn discover_recipes(search_dirs: &[PathBuf]) -> Result<Vec<RecipeInfo>> {
    let mut recipes = BTreeMap::<String, RecipeInfo>::new();

    for search_dir in search_dirs {
        if !search_dir.is_dir() {
            continue;
        }

        let mut yaml_paths = Vec::new();
        for entry in fs::read_dir(search_dir)
            .with_context(|| format!("failed to read {}", search_dir.display()))?
        {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("yaml") {
                yaml_paths.push(path);
            }
        }
        yaml_paths.sort();

        for path in yaml_paths {
            if let Ok(recipe) = parse_recipe_from_path(&path) {
                recipes.insert(
                    recipe.name.clone(),
                    RecipeInfo {
                        name: recipe.name,
                        description: recipe.description,
                        version: recipe.version,
                        author: recipe.author,
                        tags: recipe.tags,
                        step_count: recipe.steps.len(),
                    },
                );
            }
        }
    }

    Ok(recipes.into_values().collect())
}

pub(crate) fn filter_by_tags(recipes: Vec<RecipeInfo>, tags: &[String]) -> Vec<RecipeInfo> {
    if tags.is_empty() {
        return recipes;
    }

    recipes
        .into_iter()
        .filter(|recipe| {
            let recipe_tags: BTreeSet<&str> = recipe.tags.iter().map(String::as_str).collect();
            tags.iter().all(|tag| recipe_tags.contains(tag.as_str()))
        })
        .collect()
}

pub(crate) fn format_recipe_list(
    recipes: &[RecipeInfo],
    format: OutputFormat,
    verbose: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => Ok(serde_json::to_string_pretty(
            &recipes
                .iter()
                .map(|recipe| {
                    if verbose {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                            "version": recipe.version,
                            "author": recipe.author,
                            "tags": recipe.tags,
                            "step_count": recipe.step_count,
                        })
                    } else {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                        })
                    }
                })
                .collect::<Vec<_>>(),
        )?),
        OutputFormat::Yaml => Ok(serde_yaml::to_string(
            &recipes
                .iter()
                .map(|recipe| {
                    if verbose {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                            "version": recipe.version,
                            "author": recipe.author,
                            "tags": recipe.tags,
                            "step_count": recipe.step_count,
                        })
                    } else {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                        })
                    }
                })
                .collect::<Vec<_>>(),
        )?),
        OutputFormat::Table => {
            if recipes.is_empty() {
                return Ok("No recipes found (0 recipes)".to_string());
            }

            let mut lines = vec![
                format!("Available Recipes ({}):", recipes.len()),
                String::new(),
            ];
            for recipe in recipes {
                lines.push(format!("• {}", recipe.name));
                if !recipe.description.is_empty() {
                    lines.push(format!("  {}", recipe.description));
                }
                if verbose {
                    if !recipe.version.is_empty() {
                        lines.push(format!("  Version: {}", recipe.version));
                    }
                    if !recipe.author.is_empty() {
                        lines.push(format!("  Author: {}", recipe.author));
                    }
                    lines.push(format!("  Steps: {}", recipe.step_count));
                }
                if !recipe.tags.is_empty() {
                    lines.push(format!("  Tags: {}", recipe.tags.join(", ")));
                }
                lines.push(String::new());
            }
            Ok(lines.join("\n"))
        }
    }
}
