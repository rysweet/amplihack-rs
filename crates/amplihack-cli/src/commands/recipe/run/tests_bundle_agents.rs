//! Issue #2528 (D5, D5a, T7): with `--no-default-search` the runner finds
//! agents and sub-recipes only in the pinned bundle. Every agent step and
//! every sub-recipe step in the shipped `amplifier-bundle/recipes` must
//! therefore resolve inside the bundle alone.
//!
//! The agent check uses the runner's own `AgentResolver` with defaults off,
//! exactly as `recipe-runner-rs --agent-dir <bundle>/agents
//! --no-default-search` builds it. Values holding a `{{` template are
//! resolved at run time and are skipped.

use recipe_runner_rs::agent_resolver::AgentResolver;
use std::path::{Path, PathBuf};

fn bundle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle")
}

/// `(recipe file, step id, value)` for every step key `key` in every
/// top-level recipe, including steps nested under `steps:` of a step.
fn step_values(key: &str, only_type: Option<&str>) -> Vec<(String, String, String)> {
    fn walk(
        file: &str,
        steps: &serde_yaml::Value,
        key: &str,
        only_type: Option<&str>,
        out: &mut Vec<(String, String, String)>,
    ) {
        let Some(steps) = steps.as_sequence() else {
            return;
        };
        for step in steps {
            let id = step["id"].as_str().unwrap_or("?").to_owned();
            let type_ok = only_type.is_none_or(|t| step["type"].as_str() == Some(t));
            if type_ok && let Some(value) = step[key].as_str() {
                out.push((file.to_owned(), id, value.to_owned()));
            }
            walk(file, &step["steps"], key, only_type, out);
        }
    }
    let mut out = Vec::new();
    let recipes = bundle().join("recipes");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&recipes)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml" || e == "yml"))
        .collect();
    files.sort();
    for path in files {
        let doc: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| {
                panic!("{} does not parse: {e}", path.display());
            });
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        walk(&name, &doc["steps"], key, only_type, &mut out);
    }
    out
}

#[test]
fn every_bundled_agent_reference_resolves_in_the_bundle_alone() {
    let resolver = AgentResolver::from_dirs(vec![bundle().join("agents")], false);
    let refs = step_values("agent", None);
    assert!(!refs.is_empty(), "the bundle has agent steps");
    let unresolved: Vec<String> = refs
        .iter()
        .filter(|(_, _, agent)| !agent.contains("{{"))
        .filter(|(_, _, agent)| resolver.resolve(agent).is_err())
        .map(|(file, id, agent)| format!("{file} step {id}: {agent}"))
        .collect();
    assert!(
        unresolved.is_empty(),
        "agent references the pinned bundle cannot resolve with default search off:\n{}",
        unresolved.join("\n")
    );
}

#[test]
fn every_bundled_sub_recipe_reference_resolves_in_the_bundle_alone() {
    let recipes = bundle().join("recipes");
    let refs = step_values("recipe", Some("recipe"));
    assert!(!refs.is_empty(), "the bundle has sub-recipe steps");
    let unresolved: Vec<String> = refs
        .iter()
        .filter(|(_, _, name)| !name.contains("{{"))
        .filter(|(_, _, name)| {
            !["yaml", "yml"]
                .iter()
                .any(|ext| recipes.join(format!("{name}.{ext}")).is_file())
                && !recipes.join(name).is_file()
        })
        .map(|(file, id, name)| format!("{file} step {id}: {name}"))
        .collect();
    assert!(
        unresolved.is_empty(),
        "sub-recipes missing from the bundle:\n{}",
        unresolved.join("\n")
    );
}
