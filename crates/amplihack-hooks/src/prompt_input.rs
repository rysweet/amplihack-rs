use serde_json::Value;

pub(crate) fn extract_user_prompt(user_prompt: Option<&str>, extra: &Value) -> String {
    if let Some(prompt) = user_prompt
        && !prompt.trim().is_empty()
    {
        return prompt.to_string();
    }

    if let Some(prompt) = extra.get("prompt").and_then(Value::as_str)
        && !prompt.trim().is_empty()
    {
        return prompt.to_string();
    }

    match extra.get("userMessage") {
        Some(Value::String(prompt)) if !prompt.trim().is_empty() => prompt.clone(),
        Some(Value::Object(obj)) => obj
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::extract_user_prompt;
    use serde_json::json;

    #[test]
    fn extracts_prompt_from_priority_order() {
        assert_eq!(
            extract_user_prompt(Some("direct"), &json!({"prompt": "fallback"})),
            "direct"
        );
        assert_eq!(
            extract_user_prompt(None, &json!({"prompt": "fallback"})),
            "fallback"
        );
        assert_eq!(
            extract_user_prompt(None, &json!({"userMessage": "hello"})),
            "hello"
        );
        assert_eq!(
            extract_user_prompt(None, &json!({"userMessage": {"text": "dict hello"}})),
            "dict hello"
        );
    }
}
