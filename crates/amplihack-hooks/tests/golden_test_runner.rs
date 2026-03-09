//! Golden file test runner for hook parity testing.
//!
//! Reads `.input.json` / `.expected.json` pairs from `tests/golden/{hook_type}/`
//! and verifies that the Rust hook produces semantically equivalent output.

use amplihack_hooks::protocol::Hook;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Result of comparing two JSON values.
#[derive(Debug)]
pub enum CompareResult {
    Match,
    Mismatch {
        path: String,
        expected: Value,
        actual: Value,
    },
}

/// Semantically compare two JSON values.
///
/// Handles: key ordering, null vs absent, whitespace in strings.
/// Does NOT handle: float precision (not needed for hooks).
pub fn compare_json(expected: &Value, actual: &Value) -> Vec<CompareResult> {
    let mut results = Vec::new();
    compare_recursive(expected, actual, "$", &mut results);
    results
}

fn compare_recursive(
    expected: &Value,
    actual: &Value,
    path: &str,
    results: &mut Vec<CompareResult>,
) {
    match (expected, actual) {
        (Value::Object(exp_map), Value::Object(act_map)) => {
            // Check all expected keys exist in actual.
            for (key, exp_val) in exp_map {
                let child_path = format!("{}.{}", path, key);
                match act_map.get(key) {
                    Some(act_val) => compare_recursive(exp_val, act_val, &child_path, results),
                    None => {
                        // null vs absent: treat absent as null.
                        if !exp_val.is_null() {
                            results.push(CompareResult::Mismatch {
                                path: child_path,
                                expected: exp_val.clone(),
                                actual: Value::Null,
                            });
                        }
                    }
                }
            }
            // Check actual doesn't have extra non-null keys.
            for (key, act_val) in act_map {
                if !exp_map.contains_key(key) && !act_val.is_null() {
                    results.push(CompareResult::Mismatch {
                        path: format!("{}.{}", path, key),
                        expected: Value::Null,
                        actual: act_val.clone(),
                    });
                }
            }
        }
        (Value::Array(exp_arr), Value::Array(act_arr)) => {
            if exp_arr.len() != act_arr.len() {
                results.push(CompareResult::Mismatch {
                    path: format!("{}.length", path),
                    expected: Value::Number(exp_arr.len().into()),
                    actual: Value::Number(act_arr.len().into()),
                });
                return;
            }
            for (i, (exp_val, act_val)) in exp_arr.iter().zip(act_arr.iter()).enumerate() {
                compare_recursive(exp_val, act_val, &format!("{}[{}]", path, i), results);
            }
        }
        (Value::String(exp_s), Value::String(act_s)) => {
            // Normalize whitespace for comparison.
            let exp_norm: String = exp_s.split_whitespace().collect::<Vec<_>>().join(" ");
            let act_norm: String = act_s.split_whitespace().collect::<Vec<_>>().join(" ");
            if exp_norm != act_norm {
                results.push(CompareResult::Mismatch {
                    path: path.to_string(),
                    expected: expected.clone(),
                    actual: actual.clone(),
                });
            }
        }
        _ => {
            if expected != actual {
                results.push(CompareResult::Mismatch {
                    path: path.to_string(),
                    expected: expected.clone(),
                    actual: actual.clone(),
                });
            }
        }
    }
}

/// Discover all golden file test cases in a directory.
///
/// Returns pairs of (input_path, expected_path).
pub fn discover_golden_files(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();

    if !dir.exists() {
        return pairs;
    }

    let entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|_| panic!("Cannot read golden file dir: {}", dir.display()))
        .filter_map(Result::ok)
        .collect();

    for entry in &entries {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(".input.json") {
            let stem = name.trim_end_matches(".input.json");
            let expected_path = dir.join(format!("{}.expected.json", stem));
            if expected_path.exists() {
                pairs.push((path.clone(), expected_path));
            }
        }
    }

    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// Run a hook against all golden files in a directory.
pub fn run_golden_tests<H: Hook>(hook: &H, dir: &Path) -> Vec<GoldenTestResult> {
    let pairs = discover_golden_files(dir);
    let mut results = Vec::new();

    for (input_path, expected_path) in &pairs {
        let input_json = fs::read_to_string(input_path)
            .unwrap_or_else(|e| panic!("Cannot read {}: {}", input_path.display(), e));
        let expected_json = fs::read_to_string(expected_path)
            .unwrap_or_else(|e| panic!("Cannot read {}: {}", expected_path.display(), e));

        let input: amplihack_types::HookInput = match serde_json::from_str(&input_json) {
            Ok(i) => i,
            Err(e) => {
                results.push(GoldenTestResult {
                    name: input_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    passed: false,
                    mismatches: vec![format!("Failed to parse input: {}", e)],
                });
                continue;
            }
        };

        let expected: Value = match serde_json::from_str(&expected_json) {
            Ok(e) => e,
            Err(e) => {
                results.push(GoldenTestResult {
                    name: expected_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    passed: false,
                    mismatches: vec![format!("Failed to parse expected: {}", e)],
                });
                continue;
            }
        };

        let actual = match hook.process(input) {
            Ok(v) => v,
            Err(e) => {
                results.push(GoldenTestResult {
                    name: input_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    passed: false,
                    mismatches: vec![format!("Hook returned error: {}", e)],
                });
                continue;
            }
        };

        let comparison = compare_json(&expected, &actual);
        let mismatches: Vec<String> = comparison
            .iter()
            .filter_map(|r| match r {
                CompareResult::Match => None,
                CompareResult::Mismatch {
                    path,
                    expected,
                    actual,
                } => Some(format!(
                    "at {}: expected={}, actual={}",
                    path, expected, actual
                )),
            })
            .collect();

        results.push(GoldenTestResult {
            name: input_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            passed: mismatches.is_empty(),
            mismatches,
        });
    }

    results
}

/// Result of a single golden file test.
#[derive(Debug)]
pub struct GoldenTestResult {
    pub name: String,
    pub passed: bool,
    pub mismatches: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_identical() {
        let a = serde_json::json!({"key": "value", "num": 42});
        let b = serde_json::json!({"key": "value", "num": 42});
        let results = compare_json(&a, &b);
        assert!(results.iter().all(|r| matches!(r, CompareResult::Match)));
    }

    #[test]
    fn compare_different_key_order() {
        let a = serde_json::json!({"a": 1, "b": 2});
        let b = serde_json::json!({"b": 2, "a": 1});
        let results = compare_json(&a, &b);
        assert!(results.iter().all(|r| matches!(r, CompareResult::Match)));
    }

    #[test]
    fn compare_null_vs_absent() {
        let a = serde_json::json!({"key": null});
        let b = serde_json::json!({});
        let results = compare_json(&a, &b);
        assert!(results.iter().all(|r| matches!(r, CompareResult::Match)));
    }

    #[test]
    fn compare_different_values() {
        let a = serde_json::json!({"key": "hello"});
        let b = serde_json::json!({"key": "world"});
        let results = compare_json(&a, &b);
        assert!(
            results
                .iter()
                .any(|r| matches!(r, CompareResult::Mismatch { .. }))
        );
    }

    #[test]
    fn compare_whitespace_normalization() {
        let a = serde_json::json!({"msg": "hello  world"});
        let b = serde_json::json!({"msg": "hello world"});
        let results = compare_json(&a, &b);
        assert!(results.iter().all(|r| matches!(r, CompareResult::Match)));
    }

    #[test]
    fn compare_empty_objects() {
        let a = serde_json::json!({});
        let b = serde_json::json!({});
        let results = compare_json(&a, &b);
        assert!(results.is_empty());
    }
}
