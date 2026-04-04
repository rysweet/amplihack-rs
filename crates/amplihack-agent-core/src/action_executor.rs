//! Concrete [`ActionExecutor`] implementation with a dynamic tool registry.
//!
//! Ports Python `action_executor.py` — registry pattern, safe arithmetic
//! evaluator, and standard actions (read_content, search_memory, calculate,
//! synthesize_answer).

use std::collections::HashMap;

use serde_json::Value;

use crate::agentic_loop::traits::ActionExecutor;
use crate::agentic_loop::types::ActionResult;
use crate::safe_calc::safe_eval;

/// Action function type — takes a JSON params map, returns a JSON value or error.
pub type ActionFn = Box<dyn Fn(&HashMap<String, Value>) -> Result<Value, String> + Send + Sync>;

/// Tool registry and executor for goal-seeking agents.
///
/// Provides a registry of named actions (tools) that agents can invoke.
/// Each action is a closure that takes `&HashMap<String, Value>` params
/// and returns `Result<Value, String>`.
pub struct RegistryActionExecutor {
    actions: HashMap<String, ActionFn>,
}

impl RegistryActionExecutor {
    /// Create an empty executor.
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
        }
    }

    /// Create an executor pre-loaded with the standard actions
    /// (`read_content`, `calculate`).
    pub fn with_standard_actions() -> Self {
        let mut exec = Self::new();
        exec.register("read_content", Box::new(action_read_content))
            .expect("read_content registration");
        exec.register("calculate", Box::new(action_calculate))
            .expect("calculate registration");
        exec
    }

    /// Register a named action.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is empty or already registered.
    pub fn register(&mut self, name: &str, func: ActionFn) -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("Action name cannot be empty".into());
        }
        if self.actions.contains_key(trimmed) {
            return Err(format!("Action '{trimmed}' is already registered"));
        }
        self.actions.insert(trimmed.to_string(), func);
        Ok(())
    }

    /// Check if an action is registered.
    pub fn has_action(&self, name: &str) -> bool {
        self.actions.contains_key(name)
    }
}

impl Default for RegistryActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionExecutor for RegistryActionExecutor {
    fn available_actions(&self) -> Vec<String> {
        self.actions.keys().cloned().collect()
    }

    fn execute(&self, action_name: &str, params: &HashMap<String, Value>) -> ActionResult {
        let Some(func) = self.actions.get(action_name) else {
            let available: Vec<&str> = self.actions.keys().map(String::as_str).collect();
            return ActionResult::fail(format!(
                "Action '{action_name}' not found. Available: {available:?}"
            ));
        };

        match func(params) {
            Ok(output) => ActionResult::ok(output),
            Err(e) => ActionResult::fail(format!("Action '{action_name}' failed: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Standard actions
// ---------------------------------------------------------------------------

/// Read and parse text content — returns word/char counts and a preview.
pub fn action_read_content(params: &HashMap<String, Value>) -> Result<Value, String> {
    let content = params.get("content").and_then(Value::as_str).unwrap_or("");

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::json!({
            "word_count": 0,
            "char_count": 0,
            "content": "",
        }));
    }

    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let preview: String = trimmed.chars().take(200).collect();

    Ok(serde_json::json!({
        "word_count": words.len(),
        "char_count": trimmed.len(),
        "content": trimmed,
        "preview": preview,
    }))
}

/// Safe arithmetic evaluator — supports +, -, *, /, parentheses, decimals.
///
/// Uses a recursive-descent parser (no eval) for security.
pub fn action_calculate(params: &HashMap<String, Value>) -> Result<Value, String> {
    let expression = params
        .get("expression")
        .and_then(Value::as_str)
        .unwrap_or("");

    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::json!({
            "expression": expression,
            "result": null,
            "error": "Empty expression",
        }));
    }

    // Validate: only digits, operators, parens, whitespace, decimal points.
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || " +-*/().".contains(c))
    {
        return Ok(serde_json::json!({
            "expression": trimmed,
            "result": null,
            "error": format!("Invalid characters in expression: {trimmed}"),
        }));
    }

    match safe_eval(trimmed) {
        Ok(result) => Ok(serde_json::json!({
            "expression": trimmed,
            "result": result,
            "error": null,
        })),
        Err(e) => Ok(serde_json::json!({
            "expression": trimmed,
            "result": null,
            "error": e,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- RegistryActionExecutor ----

    #[test]
    fn empty_executor_has_no_actions() {
        let exec = RegistryActionExecutor::new();
        assert!(exec.available_actions().is_empty());
    }

    #[test]
    fn register_and_execute_action() {
        let mut exec = RegistryActionExecutor::new();
        exec.register(
            "greet",
            Box::new(|params: &HashMap<String, Value>| {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("World");
                Ok(Value::String(format!("Hello {name}!")))
            }),
        )
        .unwrap();

        assert!(exec.has_action("greet"));
        let result = exec.execute(
            "greet",
            &HashMap::from([("name".into(), Value::String("Alice".into()))]),
        );
        assert!(result.success);
        assert_eq!(result.output, Value::String("Hello Alice!".into()));
    }

    #[test]
    fn execute_missing_action() {
        let exec = RegistryActionExecutor::new();
        let result = exec.execute("nonexistent", &HashMap::new());
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("not found"));
    }

    #[test]
    fn register_empty_name_fails() {
        let mut exec = RegistryActionExecutor::new();
        assert!(exec.register("", Box::new(|_| Ok(Value::Null))).is_err());
        assert!(exec.register("  ", Box::new(|_| Ok(Value::Null))).is_err());
    }

    #[test]
    fn register_duplicate_name_fails() {
        let mut exec = RegistryActionExecutor::new();
        exec.register("dup", Box::new(|_| Ok(Value::Null))).unwrap();
        assert!(exec.register("dup", Box::new(|_| Ok(Value::Null))).is_err());
    }

    #[test]
    fn action_returning_error() {
        let mut exec = RegistryActionExecutor::new();
        exec.register("fail", Box::new(|_| Err("boom".into())))
            .unwrap();
        let result = exec.execute("fail", &HashMap::new());
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("boom"));
    }

    #[test]
    fn standard_actions_registered() {
        let exec = RegistryActionExecutor::with_standard_actions();
        assert!(exec.has_action("read_content"));
        assert!(exec.has_action("calculate"));
    }

    // ---- read_content ----

    #[test]
    fn read_content_empty() {
        let result = action_read_content(&HashMap::new()).unwrap();
        assert_eq!(result["word_count"], 0);
        assert_eq!(result["char_count"], 0);
    }

    #[test]
    fn read_content_basic() {
        let params = HashMap::from([("content".into(), Value::String("hello world foo".into()))]);
        let result = action_read_content(&params).unwrap();
        assert_eq!(result["word_count"], 3);
        assert_eq!(result["char_count"], 15);
    }

    #[test]
    fn read_content_preview_truncation() {
        let long = "x".repeat(500);
        let params = HashMap::from([("content".into(), Value::String(long))]);
        let result = action_read_content(&params).unwrap();
        assert_eq!(result["preview"].as_str().unwrap().len(), 200);
    }

    // ---- calculate ----

    #[test]
    fn calculate_simple() {
        let params = HashMap::from([("expression".into(), Value::String("26 - 18".into()))]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], 8.0);
        assert!(result["error"].is_null());
    }

    #[test]
    fn calculate_multiplication() {
        let params = HashMap::from([("expression".into(), Value::String("3 * 4 + 2".into()))]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], 14.0);
    }

    #[test]
    fn calculate_parentheses() {
        let params = HashMap::from([("expression".into(), Value::String("(2 + 3) * 4".into()))]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], 20.0);
    }

    #[test]
    fn calculate_division() {
        let params = HashMap::from([("expression".into(), Value::String("10 / 4".into()))]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], 2.5);
    }

    #[test]
    fn calculate_division_by_zero() {
        let params = HashMap::from([("expression".into(), Value::String("5 / 0".into()))]);
        let result = action_calculate(&params).unwrap();
        assert!(result["result"].is_null());
        assert_eq!(result["error"].as_str().unwrap(), "Division by zero");
    }

    #[test]
    fn calculate_empty_expression() {
        let params = HashMap::from([("expression".into(), Value::String("".into()))]);
        let result = action_calculate(&params).unwrap();
        assert!(result["result"].is_null());
    }

    #[test]
    fn calculate_invalid_chars() {
        let params = HashMap::from([("expression".into(), Value::String("abc + 1".into()))]);
        let result = action_calculate(&params).unwrap();
        assert!(result["result"].is_null());
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("Invalid characters")
        );
    }

    #[test]
    fn calculate_negation() {
        let params = HashMap::from([("expression".into(), Value::String("-5 + 3".into()))]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], -2.0);
    }

    #[test]
    fn calculate_decimal() {
        let params = HashMap::from([("expression".into(), Value::String("3.5 * 2".into()))]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], 7.0);
    }

    #[test]
    fn calculate_nested_parens() {
        let params = HashMap::from([(
            "expression".into(),
            Value::String("((2 + 3) * (4 - 1))".into()),
        )]);
        let result = action_calculate(&params).unwrap();
        assert_eq!(result["result"], 15.0);
    }
}
