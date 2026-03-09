//! Golden file test harness for amplihack hooks.
//!
//! Discovers `.input.json` / `.expected.json` pairs under `tests/golden/hooks/`
//! and runs each through the corresponding hook's `process()` method, comparing
//! the output semantically against the expected JSON.

use amplihack_hooks::{
    Hook, PostToolUseHook, PreCompactHook, PreToolUseHook, SessionStartHook, SessionStopHook,
    StopHook, UserPromptSubmitHook,
};
use amplihack_types::HookInput;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Semantic JSON comparator
// ---------------------------------------------------------------------------

/// Compare `actual` against `expected` with wildcard support.
///
/// Rules:
/// - `"__ANY__"` in expected matches any value in actual.
/// - `"__CONTAINS__:substring"` matches if the actual string contains the substring.
/// - Missing keys in expected are ignored (don't check that field).
/// - Key ordering doesn't matter (JSON objects are unordered by spec).
/// - Arrays are compared element-wise (length must match).
fn json_matches(actual: &Value, expected: &Value, path: &str) -> Result<(), String> {
    match (actual, expected) {
        // Wildcard: any value matches.
        (_, Value::String(s)) if s == "__ANY__" => Ok(()),

        // Contains wildcard.
        (Value::String(actual_s), Value::String(exp_s)) if exp_s.starts_with("__CONTAINS__:") => {
            let needle = &exp_s["__CONTAINS__:".len()..];
            if actual_s.contains(needle) {
                Ok(())
            } else {
                Err(format!(
                    "at {path}: expected string containing \"{needle}\", got \"{actual_s}\""
                ))
            }
        }

        // Contains wildcard on non-string actual value.
        (_, Value::String(exp_s)) if exp_s.starts_with("__CONTAINS__:") => {
            let needle = &exp_s["__CONTAINS__:".len()..];
            let actual_s = actual.to_string();
            if actual_s.contains(needle) {
                Ok(())
            } else {
                Err(format!(
                    "at {path}: expected value containing \"{needle}\", got {actual}"
                ))
            }
        }

        // Object comparison: only check keys present in expected.
        (Value::Object(actual_obj), Value::Object(expected_obj)) => {
            for (key, exp_val) in expected_obj {
                match actual_obj.get(key) {
                    Some(act_val) => {
                        json_matches(act_val, exp_val, &format!("{path}.{key}"))?;
                    }
                    None => {
                        return Err(format!("at {path}: missing key \"{key}\" in actual"));
                    }
                }
            }
            Ok(())
        }

        // Array comparison: element-wise, length must match.
        (Value::Array(actual_arr), Value::Array(expected_arr)) => {
            if actual_arr.len() != expected_arr.len() {
                return Err(format!(
                    "at {path}: array length mismatch: actual={}, expected={}",
                    actual_arr.len(),
                    expected_arr.len()
                ));
            }
            for (i, (a, e)) in actual_arr.iter().zip(expected_arr.iter()).enumerate() {
                json_matches(a, e, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }

        // Scalar comparison.
        (a, e) => {
            if a == e {
                Ok(())
            } else {
                Err(format!("at {path}: actual={a}, expected={e}"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hook dispatch
// ---------------------------------------------------------------------------

/// Map a hook subdirectory name to its Hook implementation and invoke `process`.
fn run_hook(hook_type: &str, input: HookInput) -> anyhow::Result<Value> {
    match hook_type {
        "pre_tool_use" => PreToolUseHook.process(input),
        "post_tool_use" => PostToolUseHook.process(input),
        "session_start" => SessionStartHook.process(input),
        "session_stop" => SessionStopHook.process(input),
        "stop" => StopHook.process(input),
        "user_prompt_submit" => UserPromptSubmitHook.process(input),
        "pre_compact" => PreCompactHook.process(input),
        _ => anyhow::bail!("unknown hook type: {hook_type}"),
    }
}

// ---------------------------------------------------------------------------
// Test discovery and execution
// ---------------------------------------------------------------------------

/// Discover all golden test cases under a given hook directory.
fn discover_cases(hook_dir: &Path) -> Vec<(String, PathBuf, PathBuf)> {
    let mut cases = Vec::new();

    let entries = match fs::read_dir(hook_dir) {
        Ok(e) => e,
        Err(_) => return cases,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(test_name) = name.strip_suffix(".input.json")
        {
            let expected_path = hook_dir.join(format!("{test_name}.expected.json"));
            if expected_path.exists() {
                cases.push((test_name.to_string(), path.clone(), expected_path));
            }
        }
    }

    cases.sort_by(|a, b| a.0.cmp(&b.0));
    cases
}

/// Run all golden tests for a specific hook type.
fn run_golden_tests_for_hook(hook_type: &str) {
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("golden")
        .join("hooks")
        .join(hook_type);

    let cases = discover_cases(&golden_dir);
    if cases.is_empty() {
        // No test files → skip silently (directory exists but is empty).
        return;
    }

    let mut failures: Vec<String> = Vec::new();

    for (test_name, input_path, expected_path) in &cases {
        let input_json = fs::read_to_string(input_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", input_path.display()));
        let expected_json = fs::read_to_string(expected_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", expected_path.display()));

        let input: HookInput = match serde_json::from_str(&input_json) {
            Ok(i) => i,
            Err(e) => {
                failures.push(format!(
                    "[{hook_type}/{test_name}] input deserialization: {e}"
                ));
                continue;
            }
        };

        let expected: Value = match serde_json::from_str(&expected_json) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!(
                    "[{hook_type}/{test_name}] expected deserialization: {e}"
                ));
                continue;
            }
        };

        let actual = match run_hook(hook_type, input) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!(
                    "[{hook_type}/{test_name}] hook.process() error: {e}"
                ));
                continue;
            }
        };

        if let Err(diff) = json_matches(&actual, &expected, "$") {
            failures.push(format!(
                "[{hook_type}/{test_name}] {diff}\n  actual:   {}\n  expected: {}",
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
                serde_json::to_string_pretty(&expected).unwrap_or_default(),
            ));
        }
    }

    if !failures.is_empty() {
        let report = failures.join("\n\n");
        panic!(
            "\n{} golden test failure(s) for hook '{}':\n\n{}\n",
            failures.len(),
            hook_type,
            report
        );
    }

    eprintln!("  ✓ {hook_type}: {} cases passed", cases.len());
}

// ---------------------------------------------------------------------------
// Tests — one #[test] per hook type
// ---------------------------------------------------------------------------

#[test]
fn golden_pre_tool_use() {
    run_golden_tests_for_hook("pre_tool_use");
}

#[test]
fn golden_post_tool_use() {
    run_golden_tests_for_hook("post_tool_use");
}

#[test]
fn golden_session_start() {
    run_golden_tests_for_hook("session_start");
}

#[test]
fn golden_session_stop() {
    run_golden_tests_for_hook("session_stop");
}

#[test]
fn golden_stop() {
    run_golden_tests_for_hook("stop");
}

#[test]
fn golden_user_prompt_submit() {
    run_golden_tests_for_hook("user_prompt_submit");
}

#[test]
fn golden_pre_compact() {
    run_golden_tests_for_hook("pre_compact");
}

// ---------------------------------------------------------------------------
// Comparator unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod comparator_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_match() {
        let a = json!({"key": "value", "num": 42});
        let b = json!({"key": "value", "num": 42});
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn key_order_irrelevant() {
        let a = json!({"b": 2, "a": 1});
        let b = json!({"a": 1, "b": 2});
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn any_wildcard() {
        let a = json!({"key": "anything goes"});
        let b = json!({"key": "__ANY__"});
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn contains_wildcard() {
        let a = json!({"msg": "error: file not found"});
        let b = json!({"msg": "__CONTAINS__:file not found"});
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn contains_wildcard_fails() {
        let a = json!({"msg": "all good"});
        let b = json!({"msg": "__CONTAINS__:error"});
        assert!(json_matches(&a, &b, "$").is_err());
    }

    #[test]
    fn missing_optional_in_expected() {
        let a = json!({"key": "v", "extra": 99});
        let b = json!({"key": "v"});
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn missing_required_in_actual() {
        let a = json!({"key": "v"});
        let b = json!({"key": "v", "required": true});
        assert!(json_matches(&a, &b, "$").is_err());
    }

    #[test]
    fn nested_any() {
        let a = json!({"outer": {"inner": 42}});
        let b = json!({"outer": {"inner": "__ANY__"}});
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn array_match() {
        let a = json!([1, 2, 3]);
        let b = json!([1, 2, 3]);
        assert!(json_matches(&a, &b, "$").is_ok());
    }

    #[test]
    fn array_length_mismatch() {
        let a = json!([1, 2]);
        let b = json!([1, 2, 3]);
        assert!(json_matches(&a, &b, "$").is_err());
    }

    #[test]
    fn empty_objects_match() {
        let a = json!({});
        let b = json!({});
        assert!(json_matches(&a, &b, "$").is_ok());
    }
}
