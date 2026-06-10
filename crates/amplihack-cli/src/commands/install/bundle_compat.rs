use serde_yaml::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub(super) const REQUIRED_SMART_RECIPES: &[&str] = &[
    "smart-classify-route",
    "smart-execute-routing",
    "smart-reflect-loop",
    "smart-validate-summarize",
];

const STALE_SMART_ORCHESTRATOR_MARKERS: &[&str] = &[
    "resolve-bundle-asset helper-path",
    "importlib",
    "parse-decomposition",
    "orch_helper.py",
];

const EXECUTABLE_STEP_FIELDS: &[&str] = &["command", "script", "run"];

#[derive(Debug, Error)]
pub(super) enum BundleCompatibilityError {
    #[error(
        "framework bundle compatibility check failed: no amplifier-bundle recipes directory found under {0}"
    )]
    MissingBundle(PathBuf),
    #[error("framework bundle compatibility check failed: {0} is a symlink")]
    SymlinkedPath(PathBuf),
    #[error(
        "framework bundle compatibility check failed: missing required recipe {recipe} at {path}"
    )]
    MissingRecipe { recipe: String, path: PathBuf },
    #[error("framework bundle compatibility check failed: failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("framework bundle compatibility check failed: invalid YAML in {path}: {source}")]
    InvalidYaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error(
        "framework bundle compatibility check failed: smart-orchestrator at {path} is stale or incompatible ({marker})"
    )]
    StaleSmartOrchestrator { path: PathBuf, marker: String },
    #[error(
        "framework bundle compatibility check failed: smart-orchestrator at {path} does not reference required sub-recipe {recipe}"
    )]
    MissingSmartRecipeReference { path: PathBuf, recipe: String },
    #[error(
        "framework bundle compatibility check failed: companion recipe {recipe} at {path} is invalid: {reason}"
    )]
    InvalidCompanionRecipe {
        recipe: String,
        path: PathBuf,
        reason: String,
    },
    #[error("framework bundle compatibility check failed: missing recipe manifest at {0}")]
    MissingManifest(PathBuf),
    #[error(
        "framework bundle compatibility check failed: failed to read recipe manifest {path}: {source}"
    )]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "framework bundle compatibility check failed: invalid recipe manifest JSON in {path}: {source}"
    )]
    InvalidManifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "framework bundle compatibility check failed: recipe manifest {0} is not a JSON object"
    )]
    NonObjectManifest(PathBuf),
    #[error(
        "framework bundle compatibility check failed: recipe manifest {path} is missing required entry {recipe}"
    )]
    MissingManifestEntry { path: PathBuf, recipe: String },
    #[error(
        "framework bundle compatibility check failed: recipe manifest {path} has an invalid hash entry for {recipe}"
    )]
    InvalidManifestEntry { path: PathBuf, recipe: String },
}

pub(super) fn validate_framework_bundle_compatibility(
    root_or_bundle: &Path,
) -> Result<(), BundleCompatibilityError> {
    let bundle = resolve_bundle_root(root_or_bundle)?;
    reject_symlink(&bundle)?;
    let recipes = bundle.join("recipes");
    reject_symlink(&recipes)?;

    let smart_path = recipes.join("smart-orchestrator.yaml");
    require_recipe_file(&recipes, "smart-orchestrator")?;
    let smart = read_file(&smart_path)?;
    let smart_yaml = parse_yaml(&smart_path, &smart)?;
    reject_stale_smart_orchestrator(&smart_path, &smart_yaml)?;
    let smart_recipe_refs = top_level_recipe_steps(&smart_yaml);

    for &recipe in REQUIRED_SMART_RECIPES {
        let companion_path = require_recipe_file(&recipes, recipe)?;
        let companion = read_file(&companion_path)?;
        let companion_yaml = parse_yaml(&companion_path, &companion)?;
        validate_companion_recipe_contract(&companion_path, recipe, &companion_yaml)?;
        if !smart_recipe_refs.contains(recipe) {
            return Err(BundleCompatibilityError::MissingSmartRecipeReference {
                path: smart_path.clone(),
                recipe: recipe.to_string(),
            });
        }
    }

    validate_recipe_manifest(&recipes)?;
    Ok(())
}

pub(super) fn validate_staged_framework_bundle(
    bundle: &Path,
) -> Result<(), BundleCompatibilityError> {
    validate_framework_bundle_compatibility(bundle)
}

#[cfg(test)]
fn is_compatible_framework_bundle(root_or_bundle: &Path) -> bool {
    validate_framework_bundle_compatibility(root_or_bundle).is_ok()
}

fn resolve_bundle_root(root_or_bundle: &Path) -> Result<PathBuf, BundleCompatibilityError> {
    if root_or_bundle.join("recipes").is_dir() {
        return Ok(root_or_bundle.to_path_buf());
    }

    let nested = root_or_bundle.join("amplifier-bundle");
    if nested.join("recipes").is_dir() {
        return Ok(nested);
    }

    Err(BundleCompatibilityError::MissingBundle(
        root_or_bundle.to_path_buf(),
    ))
}

fn reject_symlink(path: &Path) -> Result<(), BundleCompatibilityError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| BundleCompatibilityError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(BundleCompatibilityError::SymlinkedPath(path.to_path_buf()));
    }
    Ok(())
}

fn require_recipe_file(recipes: &Path, recipe: &str) -> Result<PathBuf, BundleCompatibilityError> {
    let path = recipes.join(format!("{recipe}.yaml"));
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| BundleCompatibilityError::MissingRecipe {
            recipe: recipe.to_string(),
            path: path.clone(),
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BundleCompatibilityError::MissingRecipe {
            recipe: recipe.to_string(),
            path,
        });
    }
    Ok(path)
}

fn read_file(path: &Path) -> Result<String, BundleCompatibilityError> {
    fs::read_to_string(path).map_err(|source| BundleCompatibilityError::ReadFile {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_yaml(path: &Path, raw: &str) -> Result<Value, BundleCompatibilityError> {
    serde_yaml::from_str(raw).map_err(|source| BundleCompatibilityError::InvalidYaml {
        path: path.to_path_buf(),
        source,
    })
}

fn reject_stale_smart_orchestrator(
    path: &Path,
    yaml: &Value,
) -> Result<(), BundleCompatibilityError> {
    let Some(Value::Sequence(steps)) = mapping_value(yaml, "steps") else {
        return Ok(());
    };

    for step in steps {
        let Value::Mapping(mapping) = step else {
            continue;
        };
        for field in EXECUTABLE_STEP_FIELDS {
            let Some(executable) = mapping_string(mapping, field) else {
                continue;
            };
            for marker in STALE_SMART_ORCHESTRATOR_MARKERS {
                if executable.contains(marker) {
                    return Err(BundleCompatibilityError::StaleSmartOrchestrator {
                        path: path.to_path_buf(),
                        marker: (*marker).to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn top_level_recipe_steps(value: &Value) -> BTreeSet<&str> {
    let mut references = BTreeSet::new();
    let Some(Value::Sequence(steps)) = mapping_value(value, "steps") else {
        return references;
    };

    for step in steps {
        let Value::Mapping(mapping) = step else {
            continue;
        };
        if mapping_string(mapping, "type") != Some("recipe") {
            continue;
        }
        if let Some(recipe) = mapping_string(mapping, "recipe") {
            references.insert(recipe);
        }
    }

    references
}

fn validate_companion_recipe_contract(
    path: &Path,
    recipe: &str,
    value: &Value,
) -> Result<(), BundleCompatibilityError> {
    let Value::Mapping(mapping) = value else {
        return Err(invalid_companion_recipe(
            path,
            recipe,
            "recipe YAML root must be a mapping",
        ));
    };
    match mapping_string(mapping, "name") {
        Some(name) if name == recipe => {}
        Some(name) => {
            return Err(invalid_companion_recipe(
                path,
                recipe,
                format!("name is {name:?}, expected {recipe:?}"),
            ));
        }
        None => {
            return Err(invalid_companion_recipe(
                path,
                recipe,
                "missing required name field",
            ));
        }
    }

    match mapping.get(Value::String("steps".to_string())) {
        Some(Value::Sequence(steps)) if !steps.is_empty() => Ok(()),
        Some(Value::Sequence(_)) => Err(invalid_companion_recipe(
            path,
            recipe,
            "steps must contain at least one step",
        )),
        Some(_) => Err(invalid_companion_recipe(
            path,
            recipe,
            "steps must be a sequence",
        )),
        None => Err(invalid_companion_recipe(
            path,
            recipe,
            "missing required steps field",
        )),
    }
}

fn invalid_companion_recipe(
    path: &Path,
    recipe: &str,
    reason: impl Into<String>,
) -> BundleCompatibilityError {
    BundleCompatibilityError::InvalidCompanionRecipe {
        recipe: recipe.to_string(),
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn mapping_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let Value::Mapping(mapping) = value else {
        return None;
    };
    mapping.iter().find_map(|(candidate, value)| {
        if matches!(candidate, Value::String(candidate) if candidate == key) {
            Some(value)
        } else {
            None
        }
    })
}

fn mapping_string<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    mapping.iter().find_map(|(candidate, value)| {
        if matches!(candidate, Value::String(candidate) if candidate == key)
            && let Value::String(value) = value
        {
            Some(value.as_str())
        } else {
            None
        }
    })
}

fn validate_recipe_manifest(recipes: &Path) -> Result<(), BundleCompatibilityError> {
    let path = recipes.join("_recipe_manifest.json");
    if !path.is_file() {
        return Err(BundleCompatibilityError::MissingManifest(path));
    }
    let raw =
        fs::read_to_string(&path).map_err(|source| BundleCompatibilityError::ReadManifest {
            path: path.clone(),
            source,
        })?;
    let manifest: serde_json::Value =
        serde_json::from_str(&raw).map_err(|source| BundleCompatibilityError::InvalidManifest {
            path: path.clone(),
            source,
        })?;
    let Some(object) = manifest.as_object() else {
        return Err(BundleCompatibilityError::NonObjectManifest(path));
    };
    for recipe in
        std::iter::once("smart-orchestrator").chain(REQUIRED_SMART_RECIPES.iter().copied())
    {
        let Some(value) = object.get(recipe) else {
            return Err(BundleCompatibilityError::MissingManifestEntry {
                path: path.clone(),
                recipe: recipe.to_string(),
            });
        };
        if !matches!(value, serde_json::Value::String(hash) if !hash.trim().is_empty()) {
            return Err(BundleCompatibilityError::InvalidManifestEntry {
                path: path.clone(),
                recipe: recipe.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write_compatible_bundle(bundle: &Path) {
        let recipes = bundle.join("recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(recipes.join("smart-orchestrator.yaml"), compatible_smart()).unwrap();
        for recipe in REQUIRED_SMART_RECIPES {
            fs::write(
                recipes.join(format!("{recipe}.yaml")),
                format!("name: \"{recipe}\"\nsteps:\n  - id: smoke\n    type: bash\n    command: 'true'\n"),
            )
            .unwrap();
        }
        fs::write(
            recipes.join("_recipe_manifest.json"),
            r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-execute-routing": "11612506ae846a47",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
        )
        .unwrap();
    }

    fn compatible_smart() -> &'static str {
        r#"name: "smart-orchestrator"
description: "Composable smart task orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
"#
    }

    fn stale_monolithic_smart() -> &'static str {
        r#"name: "smart-orchestrator"
description: "old monolithic smart task orchestrator"
steps:
  - id: "parse-decomposition"
    type: "shell"
    command: |
      HELPER="$(amplihack resolve-bundle-asset helper-path)"
      python3 - <<'PY'
      import importlib
      module = importlib.import_module("orch_helper")
      module.parse_decomposition()
      PY
"#
    }

    #[test]
    fn accepts_current_composable_smart_orchestrator_contract() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);

        validate_framework_bundle_compatibility(&bundle)
            .expect("current composable smart-orchestrator bundle must be accepted");
        validate_staged_framework_bundle(&bundle)
            .expect("staged compatible smart-orchestrator bundle must be accepted");
        assert!(
            is_compatible_framework_bundle(&bundle),
            "compatibility predicate must accept a composable smart-orchestrator with all companion recipes"
        );
    }

    #[test]
    fn rejects_stale_monolithic_smart_orchestrator_using_helper_path_importlib() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-orchestrator.yaml"),
            stale_monolithic_smart(),
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("stale monolithic smart-orchestrator must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("smart-orchestrator")
                && (msg.contains("stale")
                    || msg.contains("incompatible")
                    || msg.contains("resolve-bundle-asset helper-path")
                    || msg.contains("importlib")),
            "error must make the stale smart-orchestrator incompatibility actionable, got: {msg}"
        );
        assert!(
            !is_compatible_framework_bundle(&bundle),
            "compatibility predicate must not accept the old monolithic smart-orchestrator"
        );
    }

    #[test]
    fn rejects_missing_required_companion_recipe() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::remove_file(bundle.join("recipes/smart-reflect-loop.yaml")).unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("missing required smart sub-recipe must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("smart-reflect-loop") && msg.contains("recipe"),
            "error must name the missing companion recipe, got: {msg}"
        );
    }

    #[test]
    fn rejects_empty_required_companion_recipe_steps() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-classify-route.yaml"),
            "name: \"smart-classify-route\"\nsteps: []\n",
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("empty companion recipe must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("smart-classify-route") && msg.contains("steps"),
            "error must name empty companion recipe steps, got: {msg}"
        );
    }

    #[test]
    fn rejects_wrong_required_companion_recipe_name() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-execute-routing.yaml"),
            "name: \"wrong-recipe\"\nsteps:\n  - id: smoke\n    type: bash\n    command: 'true'\n",
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("wrong-name companion recipe must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("smart-execute-routing") && msg.contains("wrong-recipe"),
            "error must name wrong companion recipe name, got: {msg}"
        );
    }

    #[test]
    fn rejects_missing_sub_recipe_reference_even_when_file_exists() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-orchestrator.yaml"),
            compatible_smart().replace(
                "recipe: \"smart-execute-routing\"",
                "recipe: \"default-workflow\"",
            ),
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("smart-orchestrator must reference every required phase recipe");
        let msg = err.to_string();
        assert!(
            msg.contains("smart-execute-routing") && msg.contains("smart-orchestrator"),
            "error must name the missing sub-recipe reference, got: {msg}"
        );
    }

    #[test]
    fn rejects_required_recipe_names_only_in_metadata_or_context() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-orchestrator.yaml"),
            r#"name: "smart-orchestrator"
description: "Recipe names below are inert metadata, not executable recipe steps"
metadata:
  recipes:
    - recipe: "smart-classify-route"
    - recipe: "smart-execute-routing"
context:
  smart_reflect_loop:
    recipe: "smart-reflect-loop"
  smart_validate_summarize:
    recipe: "smart-validate-summarize"
steps:
  - id: "metadata-only"
    type: "bash"
    command: "echo metadata only"
"#,
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("required smart sub-recipes must be top-level type: recipe steps");
        let msg = err.to_string();
        assert!(
            msg.contains("smart-classify-route") && msg.contains("smart-orchestrator"),
            "metadata/context recipe keys must not satisfy the structural recipe-step contract, got: {msg}"
        );
    }

    #[test]
    fn accepts_stale_marker_text_in_comments_and_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-orchestrator.yaml"),
            format!(
                r#"{}
# Historical note only: resolve-bundle-asset helper-path importlib parse-decomposition orch_helper.py
metadata:
  migration_note: "Old text resolve-bundle-asset helper-path importlib parse-decomposition orch_helper.py is inert"
"#,
                compatible_smart()
            ),
        )
        .unwrap();

        validate_framework_bundle_compatibility(&bundle)
            .expect("comments and inert metadata must not trip stale executable marker checks");
    }

    #[test]
    fn rejects_executable_stale_orch_helper_py_reference() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/smart-orchestrator.yaml"),
            format!(
                r#"{}
  - id: "old-helper-script"
    type: "bash"
    command: "python3 amplifier-bundle/tools/orch_helper.py"
"#,
                compatible_smart()
            ),
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("executable orch_helper.py references must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("orch_helper.py") && msg.contains("smart-orchestrator"),
            "error must name the stale executable helper marker, got: {msg}"
        );
    }

    #[test]
    fn rejects_manifest_missing_required_smart_recipe_entries() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::write(
            bundle.join("recipes/_recipe_manifest.json"),
            r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
        )
        .unwrap();

        let err = validate_framework_bundle_compatibility(&bundle)
            .expect_err("recipe manifest must cover every required smart-orchestrator recipe");
        let msg = err.to_string();
        assert!(
            msg.contains("_recipe_manifest.json") && msg.contains("smart-execute-routing"),
            "error must name the missing manifest entry, got: {msg}"
        );
    }
}
