//! Native recipe read commands (`list`, `show`, `validate`).

use crate::command_error::exit_error;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod list;
pub mod run;
pub mod show_validate;

pub use list::run_list;
pub use run::run_recipe;
pub use show_validate::{run_show, run_validate};

pub(crate) const MAX_YAML_SIZE_BYTES: usize = 1_000_000;
const RECIPE_FILE_EXTENSIONS: &[&str] = &["yaml", "yml"];

#[derive(Debug, Clone, Copy)]
pub(crate) enum OutputFormat {
    Table,
    Json,
    Yaml,
}

impl OutputFormat {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            other => anyhow::bail!("Invalid format: {other}. Must be table, json, or yaml"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecipeDoc {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default = "default_version")]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) author: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) context: BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) steps: Vec<RawStep>,
    #[serde(default)]
    pub(crate) recursion: Option<Value>,
    #[serde(default)]
    pub(crate) output: Option<Value>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawStep {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) step_type: Option<String>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) agent: Option<String>,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    #[serde(default)]
    pub(crate) output: Option<String>,
    #[serde(default)]
    pub(crate) condition: Option<String>,
    #[serde(default)]
    pub(crate) parse_json: bool,
    #[serde(default)]
    pub(crate) mode: Option<String>,
    #[serde(default)]
    pub(crate) working_dir: Option<String>,
    #[serde(default)]
    pub(crate) timeout: Option<i64>,
    #[serde(default)]
    pub(crate) auto_stage: Option<bool>,
    #[serde(default)]
    pub(crate) recipe: Option<String>,
    #[serde(default, rename = "context")]
    pub(crate) sub_context: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RecipeInfo {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) version: String,
    pub(crate) author: String,
    pub(crate) tags: Vec<String>,
    pub(crate) step_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecipeRunResult {
    pub(crate) recipe_name: String,
    pub(crate) success: bool,
    #[serde(default)]
    pub(crate) step_results: Vec<RecipeRunStepResult>,
    #[serde(default)]
    pub(crate) context: JsonMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecipeRunStepResult {
    pub(crate) step_id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) output: String,
    #[serde(default)]
    pub(crate) error: String,
}

fn resolve_path_from(base_dir: &Path, path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let display = path.display().to_string();
    if display.trim().is_empty() {
        anyhow::bail!("Path cannot be empty");
    }

    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(base_dir.join(path))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|existing| existing == &candidate) {
        paths.push(candidate);
    }
}

fn resolve_env_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }

    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path,
    }
}

fn amplihack_home_recipe_dir() -> Option<PathBuf> {
    let raw = std::env::var_os("AMPLIHACK_HOME")?;
    if raw.is_empty() {
        return None;
    }

    let amplihack_home = resolve_env_path(PathBuf::from(&raw));
    if !amplihack_home.is_dir() {
        tracing::warn!(
            amplihack_home = %amplihack_home.display(),
            "AMPLIHACK_HOME set but resolved path is not a directory; ignoring for recipe discovery"
        );
        return None;
    }

    let candidate = amplihack_home.join("amplifier-bundle").join("recipes");
    if candidate.is_dir() {
        return Some(candidate);
    }

    tracing::warn!(
        amplihack_home = %amplihack_home.display(),
        searched = %candidate.display(),
        "AMPLIHACK_HOME root does not contain a usable amplifier-bundle/recipes directory; ignoring for recipe discovery"
    );
    None
}

pub(crate) fn recipe_search_dirs(
    recipe_dir: Option<&str>,
    base_dir: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let base_dir = resolve_path_from(&cwd, base_dir)?;

    if let Some(dir) = recipe_dir {
        return Ok(vec![resolve_path_from(&base_dir, dir)?]);
    }

    let mut dirs = Vec::new();
    if let Some(repo_root) = repo_root_from(&base_dir) {
        push_unique_path(
            &mut dirs,
            repo_root.join("amplifier-bundle").join("recipes"),
        );
    }

    if let Some(amplihack_home_dir) = amplihack_home_recipe_dir() {
        push_unique_path(&mut dirs, amplihack_home_dir);
    }

    push_unique_path(
        &mut dirs,
        home_dir()?
            .join(".amplihack")
            .join(".claude")
            .join("recipes"),
    );
    push_unique_path(&mut dirs, base_dir.join("amplifier-bundle").join("recipes"));
    push_unique_path(
        &mut dirs,
        base_dir
            .join("src")
            .join("amplihack")
            .join("amplifier-bundle")
            .join("recipes"),
    );
    push_unique_path(&mut dirs, base_dir.join(".claude").join("recipes"));

    Ok(dirs)
}

pub(crate) fn repo_root_from(base_dir: &Path) -> Option<PathBuf> {
    let mut current = base_dir.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

pub(crate) fn find_recipe_path(name: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    for search_dir in search_dirs {
        for extension in RECIPE_FILE_EXTENSIONS {
            let candidate = search_dir.join(format!("{name}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn looks_like_recipe_path(input: &str) -> bool {
    let candidate = Path::new(input);
    candidate.components().count() > 1
        || candidate
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| RECIPE_FILE_EXTENSIONS.contains(&value))
}

pub(crate) fn resolve_recipe_path(input: &str, working_dir: impl AsRef<Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let working_dir = resolve_path_from(&cwd, working_dir)?;
    let candidate = Path::new(input);

    if candidate.is_absolute() {
        return Ok(candidate.to_path_buf());
    }

    if looks_like_recipe_path(input) {
        // Bare filename (no directory separator): try CWD first, then working_dir.
        // This matches Python behavior: recipe file is resolved relative to CWD;
        // --working-dir is only used for subprocess execution context.
        let is_bare = candidate.components().count() == 1;
        if is_bare {
            let cwd_candidate = cwd.join(candidate);
            if cwd_candidate.is_file() {
                return Ok(cwd_candidate);
            }
        }
        // Paths with directory separators (e.g. recipes/custom.yaml) are resolved
        // relative to working_dir, as are bare filenames not found in CWD.
        return resolve_path_from(&working_dir, candidate);
    }

    let search_dirs = recipe_search_dirs(None, &working_dir)?;
    if let Some(resolved) = find_recipe_path(input, &search_dirs) {
        return Ok(resolved);
    }

    anyhow::bail!(
        "Recipe not found by name: {input}. Searched: {}",
        search_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

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

    // Check that name is not null (Python: `if not name:` catches None)
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

    // Validate steps at the raw YAML level before serde deserialization,
    // so we produce Python-matching error messages for null/missing id fields.
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

pub(crate) fn require_field(mapping: &Mapping, key: &str, message: &str) -> Result<()> {
    if mapping.contains_key(Value::String(key.to_string())) {
        return Ok(());
    }
    anyhow::bail!(message.to_string())
}

pub(crate) fn validate_path(path: impl AsRef<Path>, must_exist: bool) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let resolved = resolve_path_from(&cwd, path)?;

    if must_exist && !resolved.exists() {
        anyhow::bail!("Path does not exist: {}", resolved.display());
    }

    Ok(resolved)
}

pub(crate) fn default_version() -> String {
    "1.0.0".to_string()
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|value| !value.as_os_str().is_empty())
        .context("HOME is not set")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::recipe::list::{discover_recipes, filter_by_tags, format_recipe_list};
    use crate::commands::recipe::show_validate::{format_recipe_details, format_validation_result};
    use std::ffi::OsString;
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set_path(name: &'static str, value: impl AsRef<Path>) -> Self {
            let previous = std::env::var_os(name);
            unsafe { std::env::set_var(name, value.as_ref()) };
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var(self.name, value) },
                None => unsafe { std::env::remove_var(self.name) },
            }
        }
    }

    #[derive(Clone, Default)]
    struct CapturingSubscriber {
        messages: Arc<Mutex<Vec<String>>>,
        next_span_id: Arc<AtomicU64>,
    }

    impl Subscriber for CapturingSubscriber {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() <= tracing::Level::WARN
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(self.next_span_id.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldRecorder::default();
            event.record(&mut visitor);
            let mut line = event.metadata().level().to_string();
            if !visitor.fields.is_empty() {
                line.push(' ');
                line.push_str(&visitor.fields.join(" "));
            }
            self.messages
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(line);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}

        fn register_callsite(
            &self,
            _metadata: &'static Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }

        fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
            Some(tracing::metadata::LevelFilter::WARN)
        }
    }

    #[derive(Default)]
    struct FieldRecorder {
        fields: Vec<String>,
    }

    impl Visit for FieldRecorder {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }

    fn capture_warn_logs<T>(operation: impl FnOnce() -> T) -> (T, Vec<String>) {
        let subscriber = CapturingSubscriber::default();
        let messages = Arc::clone(&subscriber.messages);
        let result = tracing::subscriber::with_default(subscriber, operation);
        let captured = messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        (result, captured)
    }

    #[test]
    fn parse_recipe_validates_required_fields() {
        let err = parse_recipe_text("description: nope\nsteps: []").unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn parse_recipe_rejects_duplicate_step_ids() {
        let err = parse_recipe_text(
            r#"
name: demo
steps:
  - id: same
    command: echo one
  - id: same
    command: echo two
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Duplicate step id"));
    }

    #[test]
    fn resolve_recipe_path_finds_named_recipe_in_project_local_dir() {
        let temp = tempdir().unwrap();
        let recipes_dir = temp.path().join(".claude").join("recipes");
        fs::create_dir_all(&recipes_dir).unwrap();
        let recipe_path = recipes_dir.join("project-only-recipe.yaml");
        fs::write(
            &recipe_path,
            "name: project-only-recipe\nsteps:\n  - id: demo\n    type: bash\n    command: echo hi\n",
        )
        .unwrap();

        let resolved = resolve_recipe_path("project-only-recipe", temp.path()).unwrap();

        assert_eq!(resolved, recipe_path);
    }

    #[test]
    fn resolve_recipe_path_prefers_cwd_for_bare_yaml_filename_with_working_dir_override() {
        let _cwd_guard = crate::test_support::cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempdir().unwrap();

        // demo.yaml lives in CWD (the temp dir), not in working_dir (sandbox/).
        let cwd_recipe = temp.path().join("demo.yaml");
        fs::write(
            &cwd_recipe,
            "name: demo\nsteps:\n  - id: step-1\n    command: pwd\n",
        )
        .unwrap();

        // working_dir is a subdirectory that does NOT contain demo.yaml.
        let working_dir = temp.path().join("sandbox");
        fs::create_dir_all(&working_dir).unwrap();

        // Set CWD to temp root so the bare filename resolves there.
        let previous_cwd = crate::test_support::set_cwd(temp.path()).unwrap();

        let resolved = resolve_recipe_path("demo.yaml", &working_dir).unwrap();

        crate::test_support::restore_cwd(&previous_cwd).unwrap();

        assert_eq!(
            resolved, cwd_recipe,
            "bare yaml filename should resolve from CWD, not from working_dir"
        );
    }

    #[test]
    fn resolve_recipe_path_uses_working_dir_for_relative_yaml_paths() {
        let temp = tempdir().unwrap();
        let working_dir = temp.path().join("project");
        let nested_dir = working_dir.join("recipes");
        fs::create_dir_all(&nested_dir).unwrap();
        let recipe_path = nested_dir.join("custom.yaml");
        fs::write(
            &recipe_path,
            "name: custom\nsteps:\n  - id: demo\n    type: bash\n    command: echo hi\n",
        )
        .unwrap();

        let resolved = resolve_recipe_path("recipes/custom.yaml", &working_dir).unwrap();

        assert_eq!(resolved, recipe_path);
    }

    #[test]
    fn resolve_recipe_path_searches_repo_root_from_nested_working_dir() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".git")).unwrap();
        let nested_working_dir = temp.path().join("nested").join("project");
        fs::create_dir_all(&nested_working_dir).unwrap();
        let recipes_dir = temp.path().join("amplifier-bundle").join("recipes");
        fs::create_dir_all(&recipes_dir).unwrap();
        let recipe_path = recipes_dir.join("smart-orchestrator.yaml");
        fs::write(&recipe_path, "name: smart-orchestrator\nsteps:\n  - id: demo\n    type: bash\n    command: echo hi\n").unwrap();

        let resolved = resolve_recipe_path("smart-orchestrator", &nested_working_dir).unwrap();

        assert_eq!(resolved, recipe_path);
    }

    #[test]
    fn resolve_recipe_path_searches_amplihack_home_bundle_dir() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempdir().unwrap();
        let amplihack_home = temp.path().join("amplihack-home");
        let recipes_dir = amplihack_home.join("amplifier-bundle").join("recipes");
        fs::create_dir_all(&recipes_dir).unwrap();
        let recipe_path = recipes_dir.join("home-bundle-recipe.yaml");
        fs::write(
            &recipe_path,
            "name: home-bundle-recipe\nsteps:\n  - id: demo\n    type: bash\n    command: echo hi\n",
        )
        .unwrap();
        let previous = std::env::var_os("AMPLIHACK_HOME");
        unsafe { std::env::set_var("AMPLIHACK_HOME", &amplihack_home) };

        let resolved = resolve_recipe_path("home-bundle-recipe", temp.path()).unwrap();

        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(resolved, recipe_path);
    }

    #[test]
    fn amplihack_home_recipe_dir_warns_for_non_directory_root_with_resolved_path() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = crate::test_support::cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempdir().unwrap();
        let invalid_root = temp.path().join("invalid-home");
        fs::write(&invalid_root, "not a directory").unwrap();
        let previous_cwd = crate::test_support::set_cwd(temp.path()).unwrap();
        let _amplihack_home = EnvVarGuard::set_path("AMPLIHACK_HOME", Path::new("invalid-home"));

        let (resolved, warnings) = capture_warn_logs(amplihack_home_recipe_dir);

        crate::test_support::restore_cwd(&previous_cwd).unwrap();

        assert!(resolved.is_none());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("resolved path is not a directory"));
        assert!(warnings[0].contains(&invalid_root.display().to_string()));
        assert!(!warnings[0].contains("amplifier-bundle/recipes"));
    }

    #[test]
    fn amplihack_home_recipe_dir_warns_when_bundle_recipes_subdir_is_missing() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = crate::test_support::cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempdir().unwrap();
        let amplihack_home = temp.path().join("amplihack-home");
        fs::create_dir_all(&amplihack_home).unwrap();
        let previous_cwd = crate::test_support::set_cwd(temp.path()).unwrap();
        let _amplihack_home = EnvVarGuard::set_path("AMPLIHACK_HOME", Path::new("amplihack-home"));
        let expected_recipe_dir = amplihack_home.join("amplifier-bundle").join("recipes");

        let (resolved, warnings) = capture_warn_logs(amplihack_home_recipe_dir);

        crate::test_support::restore_cwd(&previous_cwd).unwrap();

        assert!(resolved.is_none());
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("does not contain a usable amplifier-bundle/recipes directory")
        );
        assert!(warnings[0].contains(&amplihack_home.display().to_string()));
        assert!(warnings[0].contains(&expected_recipe_dir.display().to_string()));
    }

    #[test]
    fn list_formats_json() {
        let recipes = vec![RecipeInfo {
            name: "demo".into(),
            description: "desc".into(),
            version: "1.0.0".into(),
            author: "me".into(),
            tags: vec!["tag".into()],
            step_count: 2,
        }];
        let output = format_recipe_list(&recipes, OutputFormat::Json, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed[0]["name"], "demo");
        assert_eq!(parsed[0]["step_count"], 2);
    }

    #[test]
    fn validation_matches_python_shape() {
        let recipe = parse_recipe_text(
            r#"
name: demo
steps:
  - id: step-1
    command: echo hi
"#,
        )
        .unwrap();
        let output =
            format_validation_result(Some(&recipe), true, &[], OutputFormat::Table, true).unwrap();
        assert!(output.contains("✓ Recipe is valid"));
        assert!(output.contains("Name: demo"));
    }

    #[test]
    fn detail_table_shows_steps_and_context() {
        let recipe = parse_recipe_text(
            r#"
name: demo
description: Example
tags: [one, two]
context:
  repo_path: .
steps:
  - id: step-1
    agent: helper
    prompt: Test prompt
"#,
        )
        .unwrap();
        let output = format_recipe_details(&recipe, OutputFormat::Table, true, true).unwrap();
        assert!(output.contains("Steps (1):"));
        assert!(output.contains("Context Variables:"));
        assert!(output.contains("repo_path: ."));
    }

    #[test]
    fn discover_recipes_prefers_later_directories() {
        let temp = tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(
            first.join("demo.yaml"),
            "name: demo\nsteps:\n  - id: one\n    command: echo first\n",
        )
        .unwrap();
        fs::write(
            second.join("demo.yaml"),
            "name: demo\ndescription: override\nsteps:\n  - id: one\n    command: echo second\n",
        )
        .unwrap();

        let recipes = discover_recipes(&[first, second]).unwrap();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].description, "override");
    }

    #[test]
    fn filter_tags_uses_and_logic() {
        let recipes = vec![
            RecipeInfo {
                name: "one".into(),
                description: String::new(),
                version: String::new(),
                author: String::new(),
                tags: vec!["a".into(), "b".into()],
                step_count: 1,
            },
            RecipeInfo {
                name: "two".into(),
                description: String::new(),
                version: String::new(),
                author: String::new(),
                tags: vec!["a".into()],
                step_count: 1,
            },
        ];

        let filtered = filter_by_tags(recipes, &["a".into(), "b".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "one");
    }
}
