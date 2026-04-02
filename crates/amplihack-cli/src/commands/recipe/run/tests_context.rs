use super::*;
use std::collections::BTreeMap;

// -------------------------------------------------------------------------
// parse_context_args — key=value parsing
// -------------------------------------------------------------------------

/// Valid single key=value pair must be parsed without errors.
#[test]
fn test_parse_context_args_valid_single_pair() {
    let args = vec!["task_description=hello world".to_string()];
    let (ctx, errs) = parse_context_args(&args);
    assert!(errs.is_empty(), "No errors expected. Got: {:?}", errs);
    assert_eq!(
        ctx.get("task_description").map(String::as_str),
        Some("hello world")
    );
}

/// Multiple valid key=value pairs must all be parsed correctly.
#[test]
fn test_parse_context_args_multiple_pairs() {
    let args = vec![
        "foo=bar".to_string(),
        "baz=qux".to_string(),
        "repo_path=/tmp".to_string(),
    ];
    let (ctx, errs) = parse_context_args(&args);
    assert!(errs.is_empty(), "No errors expected. Got: {:?}", errs);
    assert_eq!(ctx.len(), 3);
    assert_eq!(ctx.get("foo").map(String::as_str), Some("bar"));
    assert_eq!(ctx.get("baz").map(String::as_str), Some("qux"));
    assert_eq!(ctx.get("repo_path").map(String::as_str), Some("/tmp"));
}

/// Empty context args must produce an empty map with no errors.
#[test]
fn test_parse_context_args_empty_input() {
    let (ctx, errs) = parse_context_args(&[]);
    assert!(errs.is_empty(), "Empty input must produce no errors");
    assert!(ctx.is_empty(), "Empty input must produce empty context map");
}

/// An arg without '=' must produce an error with a helpful message.
#[test]
fn test_parse_context_args_invalid_no_equals_sign() {
    let args = vec!["no-equals-sign".to_string()];
    let (ctx, errs) = parse_context_args(&args);
    assert_eq!(errs.len(), 1, "Exactly one error expected. Got: {:?}", errs);
    assert!(
        errs[0].contains("key=value"),
        "Error message must mention 'key=value' format. Got: {}",
        errs[0]
    );
    assert!(ctx.is_empty(), "No context should be parsed on error");
}

/// A value that itself contains '=' must be preserved correctly.
/// The split must only happen on the FIRST '='.
#[test]
fn test_parse_context_args_value_contains_equals() {
    let args = vec!["url=https://example.com?a=1&b=2".to_string()];
    let (ctx, errs) = parse_context_args(&args);
    assert!(errs.is_empty(), "No errors expected. Got: {:?}", errs);
    assert_eq!(
        ctx.get("url").map(String::as_str),
        Some("https://example.com?a=1&b=2"),
        "Value with embedded '=' must not be truncated"
    );
}

// -------------------------------------------------------------------------
// resolve_binary_path — path resolution and ~ expansion
// -------------------------------------------------------------------------

/// A path that does not exist must return None.
#[test]
fn test_resolve_binary_path_returns_none_for_nonexistent_path() {
    let result = binary::resolve_binary_path("/definitely/does/not/exist/binary");
    assert!(
        result.is_none(),
        "Non-existent path must resolve to None. Got: {:?}",
        result
    );
}

/// A bare name not in PATH must return None.
#[test]
fn test_resolve_binary_path_returns_none_for_unknown_binary_name() {
    let result = binary::resolve_binary_path("this-binary-cannot-possibly-exist-amplihack-test");
    assert!(
        result.is_none(),
        "Unknown binary name must resolve to None. Got: {:?}",
        result
    );
}

/// A well-known binary that IS in PATH must resolve to Some(path).
#[test]
#[cfg(unix)]
fn test_resolve_binary_path_finds_known_binary_in_path() {
    // `true` is guaranteed to exist on any Unix system
    let result = binary::resolve_binary_path("true");
    assert!(
        result.is_some(),
        "'true' binary must be found in PATH via resolve_binary_path"
    );
    let resolved = result.unwrap();
    assert!(
        resolved.is_file(),
        "Resolved path must point to an existing file. Got: {:?}",
        resolved
    );
}

/// A ~/... path must be expanded using the home directory.
#[test]
#[cfg(unix)]
fn test_resolve_binary_path_expands_tilde_to_home_dir() {
    // Hold the home_env_lock so that tests which temporarily override HOME
    // cannot race with this test and corrupt its view of the HOME env var.
    let _home_guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Create a temp file inside the home directory to test expansion
    let home = std::env::var("HOME").expect("HOME env var must be set");
    let temp = tempfile::NamedTempFile::new_in(&home).expect("failed to create temp file in HOME");

    // Make it executable so resolve_binary_path treats it as a file candidate
    let tilde_path = format!("~/{}", temp.path().file_name().unwrap().to_str().unwrap());

    // resolve_binary_path checks is_file(), which is true for NamedTempFile
    let result = binary::resolve_binary_path(&tilde_path);
    assert!(
        result.is_some(),
        "Tilde path '{}' must expand to HOME and resolve. Got: None",
        tilde_path
    );
    let resolved = result.unwrap();
    assert!(
        resolved.starts_with(&home),
        "Resolved path must start with HOME ({home}). Got: {:?}",
        resolved
    );
}

// -------------------------------------------------------------------------
// infer_missing_context — env var inference and default merging
// -------------------------------------------------------------------------

/// User-provided context must override recipe defaults.
#[test]
fn test_infer_missing_context_user_values_override_recipe_defaults() {
    let mut recipe_defaults = BTreeMap::new();
    recipe_defaults.insert(
        "task_description".to_string(),
        serde_yaml::Value::String("recipe default".to_string()),
    );

    let mut user_context = BTreeMap::new();
    user_context.insert("task_description".to_string(), "user override".to_string());

    let (merged, inferred) = infer_missing_context(&recipe_defaults, &user_context);

    assert_eq!(
        merged.get("task_description").map(String::as_str),
        Some("user override"),
        "User context must override recipe defaults"
    );
    assert!(
        inferred.is_empty(),
        "No inference should occur when user provides the value. Got: {:?}",
        inferred
    );
}

/// When task_description is missing from user context and AMPLIHACK_TASK_DESCRIPTION
/// env var is set, it must be inferred automatically.
#[test]
fn test_infer_missing_context_infers_task_description_from_env() {
    // SAFETY: test-only env manipulation
    unsafe { std::env::set_var("AMPLIHACK_TASK_DESCRIPTION", "from env var") };

    let mut recipe_defaults = BTreeMap::new();
    recipe_defaults.insert(
        "task_description".to_string(),
        serde_yaml::Value::String(String::new()), // empty default
    );

    let (merged, inferred) = infer_missing_context(&recipe_defaults, &BTreeMap::new());

    unsafe { std::env::remove_var("AMPLIHACK_TASK_DESCRIPTION") };

    assert_eq!(
        merged.get("task_description").map(String::as_str),
        Some("from env var"),
        "task_description must be inferred from AMPLIHACK_TASK_DESCRIPTION"
    );
    assert!(
        inferred.iter().any(|s| s.contains("task_description")),
        "Inferred list must mention task_description. Got: {:?}",
        inferred
    );
}

/// When repo_path is a required context key, it defaults to "." if no env var is set.
#[test]
fn test_infer_missing_context_repo_path_defaults_to_dot() {
    // Ensure no override is present
    unsafe { std::env::remove_var("AMPLIHACK_REPO_PATH") };

    let mut recipe_defaults = BTreeMap::new();
    recipe_defaults.insert(
        "repo_path".to_string(),
        serde_yaml::Value::String(String::new()),
    );

    let (merged, _inferred) = infer_missing_context(&recipe_defaults, &BTreeMap::new());

    assert_eq!(
        merged.get("repo_path").map(String::as_str),
        Some("."),
        "repo_path must default to '.' when AMPLIHACK_REPO_PATH is not set"
    );
}
