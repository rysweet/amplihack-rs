use serde_yaml::Value;
use std::collections::HashSet;
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
];

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
    reject_stale_smart_orchestrator(&smart_path, &smart)?;
    let smart_yaml = parse_yaml(&smart_path, &smart)?;
    let smart_recipe_refs = recipe_references(&smart_yaml);

    for &recipe in REQUIRED_SMART_RECIPES {
        let companion_path = require_recipe_file(&recipes, recipe)?;
        let companion = read_file(&companion_path)?;
        parse_yaml(&companion_path, &companion)?;
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

pub(super) fn is_compatible_framework_bundle(root_or_bundle: &Path) -> bool {
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

fn reject_stale_smart_orchestrator(path: &Path, raw: &str) -> Result<(), BundleCompatibilityError> {
    for marker in STALE_SMART_ORCHESTRATOR_MARKERS {
        if raw.contains(marker) {
            return Err(BundleCompatibilityError::StaleSmartOrchestrator {
                path: path.to_path_buf(),
                marker: (*marker).to_string(),
            });
        }
    }
    Ok(())
}

fn recipe_references(value: &Value) -> HashSet<&str> {
    let mut references = HashSet::new();
    collect_recipe_references(value, &mut references);
    references
}

fn collect_recipe_references<'a>(value: &'a Value, references: &mut HashSet<&'a str>) {
    match value {
        Value::Mapping(mapping) => {
            for (key, value) in mapping {
                if matches!(key, Value::String(k) if k == "recipe")
                    && let Value::String(recipe) = value
                {
                    references.insert(recipe.as_str());
                }
                collect_recipe_references(value, references);
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_recipe_references(item, references);
            }
        }
        _ => {}
    }
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
                format!("name: \"{recipe}\"\nsteps: []\n"),
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
