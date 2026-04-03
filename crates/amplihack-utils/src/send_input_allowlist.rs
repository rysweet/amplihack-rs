//! Allow-list for safe `send_input` patterns in outside-in test scenarios.
//!
//! Ported from `amplihack/testing/send_input_allowlist.py`.
//!
//! Security hardening for the gadugi-agentic-test YAML framework's `send_input`
//! action.  Only patterns on the allow-list can be used without explicit
//! confirmation; arbitrary values require passing `confirm = true`.
//!
//! Safe patterns are common, low-risk interaction responses:
//! - `y` / `yes` / `n` / `no` (confirmation prompts)
//! - empty / `\n` (proceed / dismiss)
//! - `q` / `quit` / `exit` (leave interactive mode)

use std::collections::HashSet;
use std::path::Path;
use thiserror::Error;

/// Environment variable pointing to a JSON file with additional safe patterns.
pub const ALLOWLIST_ENV_VAR: &str = "AMPLIHACK_SEND_INPUT_ALLOWLIST";

/// Default safe patterns (normalised: lowercase, stripped).
const DEFAULT_PATTERNS: &[&str] = &[
    "", "\n", "y", "y\n", "yes", "yes\n", "n", "n\n", "no", "no\n", "q", "q\n", "quit", "quit\n",
    "exit", "exit\n",
];

/// Error returned when a `send_input` value is not on the allow-list and
/// confirmation has not been granted.
#[derive(Debug, Error)]
#[error(
    "send_input value {value:?} is not on the safe allow-list. \
     Use --confirm to permit arbitrary input, or add the value to \
     the allow-list via the {var} environment variable.",
    var = ALLOWLIST_ENV_VAR
)]
pub struct UnsafeInputError {
    /// The rejected input value.
    pub value: String,
}

/// Return the effective allow-list (defaults + any configured extras).
///
/// Extra patterns are loaded from the JSON file pointed to by
/// [`ALLOWLIST_ENV_VAR`], if set and readable.
pub fn get_safe_patterns() -> HashSet<String> {
    let mut set: HashSet<String> = DEFAULT_PATTERNS.iter().map(|p| normalise(p)).collect();

    for extra in load_extra_patterns() {
        set.insert(normalise(&extra));
    }
    set
}

/// Return `true` if `value` matches a safe pattern.
///
/// Comparison is case-insensitive and ignores leading/trailing whitespace.
///
/// # Examples
///
/// ```
/// use amplihack_utils::send_input_allowlist::is_safe_pattern;
///
/// assert!(is_safe_pattern("y"));
/// assert!(is_safe_pattern("YES\n"));
/// assert!(!is_safe_pattern("rm -rf /"));
/// ```
pub fn is_safe_pattern(value: &str) -> bool {
    let norm = normalise(value);
    get_safe_patterns().contains(&norm)
}

/// Validate a `send_input` value against the allow-list.
///
/// When `confirm` is `true` the check is bypassed entirely.
///
/// # Errors
///
/// Returns [`UnsafeInputError`] if the value is not safe and `confirm` is
/// `false`.
pub fn validate_send_input(value: &str, confirm: bool) -> Result<(), UnsafeInputError> {
    if confirm {
        return Ok(());
    }
    if is_safe_pattern(value) {
        return Ok(());
    }
    Err(UnsafeInputError {
        value: value.to_owned(),
    })
}

/// Validate all `send_input` values in a parsed YAML scenario.
///
/// Walks the `steps` array and checks every step whose `action` is
/// `"send_input"`.
///
/// When `confirm` is `true`, unsafe values are **collected** but no error is
/// returned.  When `confirm` is `false`, the first unsafe value causes an
/// immediate error.
///
/// # Errors
///
/// Returns [`UnsafeInputError`] on the first unsafe value when `confirm` is
/// `false`.
pub fn validate_scenario_send_inputs(
    scenario: &serde_json::Value,
    confirm: bool,
) -> Result<Vec<String>, UnsafeInputError> {
    let mut unsafe_values: Vec<String> = Vec::new();
    let steps = match scenario.get("steps").and_then(|s| s.as_array()) {
        Some(arr) => arr,
        None => return Ok(unsafe_values),
    };

    for step in steps {
        let action = match step.get("action").and_then(|a| a.as_str()) {
            Some(a) => a,
            None => continue,
        };
        if action != "send_input" {
            continue;
        }

        let value = step
            .get("value")
            .map(|v| match v.as_str() {
                Some(s) => s.to_owned(),
                None => v.to_string(),
            })
            .unwrap_or_default();

        if !is_safe_pattern(&value) {
            if confirm {
                unsafe_values.push(value);
            } else {
                return Err(UnsafeInputError { value });
            }
        }
    }

    Ok(unsafe_values)
}

/// Normalise a pattern for comparison: strip whitespace, lowercase.
fn normalise(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Load extra patterns from the JSON file specified by [`ALLOWLIST_ENV_VAR`].
fn load_extra_patterns() -> Vec<String> {
    let path_str = match std::env::var(ALLOWLIST_ENV_VAR) {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };

    let path = Path::new(&path_str);
    if !path.is_file() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(serde_json::Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_safe_patterns_include_basics() {
        let safe = get_safe_patterns();
        assert!(safe.contains("y"));
        assert!(safe.contains("no"));
        assert!(safe.contains("exit"));
        assert!(safe.contains(""));
    }

    #[test]
    fn is_safe_pattern_case_insensitive() {
        assert!(is_safe_pattern("Y"));
        assert!(is_safe_pattern("YES"));
        assert!(is_safe_pattern("  yes  "));
        assert!(is_safe_pattern("Exit\n"));
    }

    #[test]
    fn unsafe_pattern_rejected() {
        assert!(!is_safe_pattern("rm -rf /"));
        assert!(!is_safe_pattern("curl http://evil.com"));
    }

    #[test]
    fn validate_send_input_ok_for_safe() {
        assert!(validate_send_input("y", false).is_ok());
        assert!(validate_send_input("no", false).is_ok());
    }

    #[test]
    fn validate_send_input_err_for_unsafe() {
        let err = validate_send_input("rm -rf /", false).unwrap_err();
        assert_eq!(err.value, "rm -rf /");
    }

    #[test]
    fn validate_send_input_bypass_with_confirm() {
        assert!(validate_send_input("rm -rf /", true).is_ok());
    }

    #[test]
    fn validate_scenario_collects_unsafe_when_confirmed() {
        let scenario = serde_json::json!({
            "steps": [
                {"action": "send_input", "value": "y"},
                {"action": "send_input", "value": "danger"},
                {"action": "click", "value": "button"},
                {"action": "send_input", "value": "also_bad"},
            ]
        });
        let unsafe_vals = validate_scenario_send_inputs(&scenario, true).unwrap();
        assert_eq!(unsafe_vals, vec!["danger", "also_bad"]);
    }

    #[test]
    fn validate_scenario_errors_on_first_unsafe() {
        let scenario = serde_json::json!({
            "steps": [
                {"action": "send_input", "value": "y"},
                {"action": "send_input", "value": "bad_cmd"},
            ]
        });
        let err = validate_scenario_send_inputs(&scenario, false).unwrap_err();
        assert_eq!(err.value, "bad_cmd");
    }
}
