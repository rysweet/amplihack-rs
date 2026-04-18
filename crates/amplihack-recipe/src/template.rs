//! Template expansion — `{{variable}}` substitution in recipe strings.
//!
//! Replaces `{{key}}` placeholders with values from a context map.
//! Supports dot-notation for nested access (e.g. `{{strategy.primary_focus}}`).
//! Undefined variables are left as-is to allow downstream resolution.

use std::collections::HashMap;

/// Expand `{{key}}` templates in a string using the given context.
///
/// Supports:
/// - Simple keys: `{{task_description}}`
/// - Dot-notation: `{{strategy.parallel_deployment.primary_focus}}`
/// - Missing keys are left unexpanded.
pub fn expand_template(template: &str, context: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next(); // consume second '{'
            let mut key = String::new();
            let mut found_close = false;
            while let Some(c2) = chars.next() {
                if c2 == '}' && chars.peek() == Some(&'}') {
                    chars.next(); // consume second '}'
                    found_close = true;
                    break;
                }
                key.push(c2);
            }
            if found_close {
                let key_trimmed = key.trim();
                if let Some(val) = context.get(key_trimmed) {
                    result.push_str(val);
                } else {
                    // Try dot-notation lookup: first segment as key
                    let resolved = resolve_dotted(key_trimmed, context);
                    if let Some(val) = resolved {
                        result.push_str(&val);
                    } else {
                        // Leave unexpanded
                        result.push_str("{{");
                        result.push_str(&key);
                        result.push_str("}}");
                    }
                }
            } else {
                // Unclosed template — emit as-is
                result.push_str("{{");
                result.push_str(&key);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Attempt to resolve a dotted key like `strategy.parallel_deployment.focus`
/// by looking up the full key first, then trying prefix combinations.
fn resolve_dotted(key: &str, context: &HashMap<String, String>) -> Option<String> {
    // Direct lookup (handles keys that contain dots literally)
    if let Some(v) = context.get(key) {
        return Some(v.clone());
    }

    // Try JSON-nested resolution: look up the first segment as a JSON value
    if let Some(dot_pos) = key.find('.')
        && let Some(root_val) = context.get(&key[..dot_pos])
        && let Ok(json_val) = serde_json::from_str::<serde_json::Value>(root_val)
        && let Some(resolved) = json_pointer(&json_val, &key[dot_pos + 1..])
    {
        return Some(resolved);
    }

    None
}

/// Navigate a JSON value using dot-notation path.
fn json_pointer(val: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = val;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Expand templates in all string fields of a step's context, command, and prompt.
pub fn expand_step_strings(
    command: Option<&str>,
    prompt: Option<&str>,
    context: &HashMap<String, String>,
) -> (Option<String>, Option<String>) {
    let expanded_cmd = command.map(|c| expand_template(c, context));
    let expanded_prompt = prompt.map(|p| expand_template(p, context));
    (expanded_cmd, expanded_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_expansion() {
        let mut ctx = HashMap::new();
        ctx.insert("name".to_string(), "world".to_string());
        assert_eq!(expand_template("Hello {{name}}!", &ctx), "Hello world!");
    }

    #[test]
    fn missing_key_left_unexpanded() {
        let ctx = HashMap::new();
        assert_eq!(
            expand_template("Hello {{missing}}!", &ctx),
            "Hello {{missing}}!"
        );
    }

    #[test]
    fn multiple_vars() {
        let mut ctx = HashMap::new();
        ctx.insert("a".to_string(), "1".to_string());
        ctx.insert("b".to_string(), "2".to_string());
        assert_eq!(expand_template("{{a}} + {{b}}", &ctx), "1 + 2");
    }

    #[test]
    fn spaces_in_braces() {
        let mut ctx = HashMap::new();
        ctx.insert("key".to_string(), "val".to_string());
        assert_eq!(expand_template("{{ key }}", &ctx), "val");
    }

    #[test]
    fn dot_notation_json() {
        let mut ctx = HashMap::new();
        ctx.insert(
            "strategy".to_string(),
            r#"{"parallel_deployment":{"primary_focus":"auth"}}"#.to_string(),
        );
        assert_eq!(
            expand_template("{{strategy.parallel_deployment.primary_focus}}", &ctx),
            "auth"
        );
    }

    #[test]
    fn no_templates() {
        let ctx = HashMap::new();
        assert_eq!(expand_template("plain text", &ctx), "plain text");
    }

    #[test]
    fn expand_step_strings_works() {
        let mut ctx = HashMap::new();
        ctx.insert("repo".to_string(), "/home/user/proj".to_string());
        let (cmd, prompt) = expand_step_strings(
            Some("cd {{repo}} && cargo test"),
            Some("Run tests in {{repo}}"),
            &ctx,
        );
        assert_eq!(cmd.as_deref(), Some("cd /home/user/proj && cargo test"));
        assert_eq!(prompt.as_deref(), Some("Run tests in /home/user/proj"));
    }
}
