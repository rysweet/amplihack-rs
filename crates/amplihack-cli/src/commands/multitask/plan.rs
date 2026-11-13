//! Normalise and validate the complete plan before preparing any workspace.
use super::{identity::WorkstreamKey, models::WorkstreamConfig};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::HashSet;

fn classification(raw: &str) -> Result<&'static str> {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .as_str()
    {
        "operations" | "ops" | "pm" | "project management" | "ops/pm" | "operations/pm" => {
            Ok("Operations")
        }
        "development" | "dev" => Ok("Development"),
        "investigation" | "research" => Ok("Investigation"),
        "q&a" | "qa" => Ok("Q&A"),
        "consensus" => Ok("Consensus"),
        _ => bail!("unsupported classification {raw:?}"),
    }
}

pub(crate) fn resolve_route(config: &mut WorkstreamConfig, default: &str) -> Result<()> {
    config.recipe = config
        .recipe
        .as_deref()
        .map(str::trim)
        .filter(|recipe| !recipe.is_empty())
        .map(str::to_owned);
    let inferred = match config.recipe.as_deref().unwrap_or(default) {
        "operations-direct" => "Operations",
        "investigation-workflow" => "Investigation",
        "qa-workflow" => "Q&A",
        "consensus-workflow" => "Consensus",
        _ => "Development",
    };
    let mut class = classification(config.classification.as_deref().unwrap_or(inferred))?;
    if class == "Operations"
        && amplihack_workflows::classifier::requires_repository_edit(&config.task)
    {
        class = "Development";
        config.recipe = None;
    }
    let recipe = match class {
        "Operations" => "operations-direct",
        "Development" => match config.recipe.as_deref().unwrap_or(default) {
            "quick-change" => "quick-change",
            _ => "default-workflow",
        },
        "Investigation" => config
            .recipe
            .as_deref()
            .filter(|r| !matches!(*r, "" | "quick-change" | "default-workflow"))
            .unwrap_or("investigation-workflow"),
        "Q&A" => config
            .recipe
            .as_deref()
            .filter(|r| !matches!(*r, "" | "quick-change" | "default-workflow"))
            .unwrap_or("qa-workflow"),
        _ => config
            .recipe
            .as_deref()
            .filter(|recipe| !matches!(*recipe, "quick-change" | "default-workflow"))
            .unwrap_or("consensus-workflow"),
    };
    ensure!(
        !recipe.is_empty()
            && !recipe.starts_with('-')
            && recipe
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-/+@:".contains(&c)),
        "invalid recipe {recipe:?}"
    );
    config.recipe = Some(recipe.to_owned());
    config.classification = Some(class.to_owned());
    Ok(())
}

/// Both decomposition output and native configuration use this boundary.
pub(crate) fn normalise(input: &Value, default: &str) -> Result<Vec<WorkstreamConfig>> {
    let leaves = input
        .as_array()
        .or_else(|| input.get("workstreams").and_then(Value::as_array))
        .filter(|leaves| !leaves.is_empty())
        .context("plan.workstreams must be a non-empty array")?;
    let top = input
        .get("classification")
        .or_else(|| input.get("task_type"))
        .cloned();
    let mut errors = Vec::new();
    let mut entries = Vec::new();
    for (index, leaf) in leaves.iter().enumerate() {
        let label = leaf
            .get("key")
            .or_else(|| leaf.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unnamed");
        let entry = (|| -> Result<WorkstreamConfig> {
            ensure!(leaf.is_object(), "leaf must be an object");
            ensure!(
                ["task", "description", "name"].iter().any(|field| leaf
                    .get(*field)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())),
                "task must not be missing or blank"
            );
            let mut value = leaf.clone();
            let name = leaf.get("name").and_then(Value::as_str).unwrap_or(label);
            if value.get("task").is_none() {
                value["task"] = leaf
                    .get("description")
                    .cloned()
                    .unwrap_or_else(|| json!(name));
            }
            // Old decompositions use name as the label and description as the task.
            if leaf.get("task").is_none() {
                value["description"] = json!(name);
            }
            if value.get("branch").is_none() {
                value["branch"] = json!(format!(
                    "feat/orch-{}-{}",
                    index + 1,
                    crate::commands::orch::slugify(name, index + 1)
                ));
            }
            if value.get("classification").is_none()
                && let Some(class) = leaf
                    .get("task_type")
                    .or_else(|| leaf.get("type"))
                    .cloned()
                    .or_else(|| top.clone())
            {
                value["classification"] = class;
            }
            if let Some(Value::Bool(b)) = value.get("is_critical_code") {
                value["is_critical_code"] = json!(b.to_string());
            }
            let mut config: WorkstreamConfig = serde_json::from_value(value).context("fields")?;
            ensure!(!config.task.trim().is_empty(), "task must not be blank");
            let issue = config.issue().context("issue")?;
            config.issue = issue.map(|n| json!(n.get())).unwrap_or(Value::Null);
            let key = super::identity::derive_key(&config, index + 1).context("key")?;
            config.key = Some(key.to_string());
            resolve_route(&mut config, default).context("classification/recipe")?;
            Ok(config)
        })();
        match entry {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(format!("workstreams[{index}] ({label}).{error:#}")),
        }
    }
    let mut keys = HashSet::new();
    for entry in &entries {
        let key = entry.key.as_deref().unwrap();
        if !keys.insert(key) {
            errors.push(format!("{key}.key: duplicate workstream identity"));
        }
    }
    for entry in &entries {
        for dependency in &entry.dependencies {
            if WorkstreamKey::new(dependency).is_err() || !keys.contains(dependency.as_str()) {
                errors.push(format!(
                    "{}.dependencies: unknown dependency {dependency:?}",
                    entry.key.as_deref().unwrap()
                ));
            }
        }
    }
    let mut done = HashSet::new();
    loop {
        let before = done.len();
        for entry in &entries {
            if entry.dependencies.iter().all(|d| done.contains(d.as_str())) {
                done.insert(entry.key.as_deref().unwrap());
            }
        }
        if done.len() == entries.len() {
            break;
        }
        if done.len() == before {
            errors.push("plan.dependencies: cycle or unresolved dependency".into());
            break;
        }
    }
    ensure!(errors.is_empty(), "invalid plan:\n{}", errors.join("\n"));
    Ok(entries)
}
