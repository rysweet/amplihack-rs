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
    let temp = tempdir().unwrap();

    let cwd_recipe = temp.path().join("demo.yaml");
    fs::write(
        &cwd_recipe,
        "name: demo\nsteps:\n  - id: step-1\n    command: pwd\n",
    )
    .unwrap();

    let working_dir = temp.path().join("sandbox");
    fs::create_dir_all(&working_dir).unwrap();

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
    fs::write(
        &recipe_path,
        "name: smart-orchestrator\nsteps:\n  - id: demo\n    type: bash\n    command: echo hi\n",
    )
    .unwrap();

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
    let temp = tempdir().unwrap();
    let invalid_root = temp.path().join("invalid-home");
    fs::write(&invalid_root, "not a directory").unwrap();
    let previous_cwd = crate::test_support::set_cwd(temp.path()).unwrap();
    let _amplihack_home = EnvVarGuard::set_path("AMPLIHACK_HOME", Path::new("invalid-home"));

    let (resolved, warnings) = capture_warn_logs(resolve::amplihack_home_recipe_dir);

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
    let temp = tempdir().unwrap();
    let amplihack_home = temp.path().join("amplihack-home");
    fs::create_dir_all(&amplihack_home).unwrap();
    let previous_cwd = crate::test_support::set_cwd(temp.path()).unwrap();
    let _amplihack_home = EnvVarGuard::set_path("AMPLIHACK_HOME", Path::new("amplihack-home"));
    let expected_recipe_dir = amplihack_home.join("amplifier-bundle").join("recipes");

    let (resolved, warnings) = capture_warn_logs(resolve::amplihack_home_recipe_dir);

    crate::test_support::restore_cwd(&previous_cwd).unwrap();

    assert!(resolved.is_none());
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("does not contain a usable amplifier-bundle/recipes directory"));
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
