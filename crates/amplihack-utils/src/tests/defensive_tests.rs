use super::*;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

// -- parse_llm_json tests ---------------------------------------------------

#[test]
fn parse_raw_object() {
    let input = r#"{"key": "value"}"#;
    let result = parse_llm_json(input).expect("should parse raw JSON");
    assert_eq!(result["key"], "value");
}

#[test]
fn parse_raw_array() {
    let input = "[1, 2, 3]";
    let result = parse_llm_json(input).expect("should parse array");
    assert_eq!(result.as_array().expect("array").len(), 3);
}

#[test]
fn parse_fenced_json() {
    let input = "Here is the result:\n```json\n{\"a\": 1}\n```\nDone.";
    let result = parse_llm_json(input).expect("should parse fenced JSON");
    assert_eq!(result["a"], 1);
}

#[test]
fn parse_fenced_no_lang() {
    let input = "Output:\n```\n{\"b\": 2}\n```";
    let result = parse_llm_json(input).expect("should parse fenced block without lang");
    assert_eq!(result["b"], 2);
}

#[test]
fn parse_embedded_object() {
    let input = "The answer is {\"x\": 42} and that is all.";
    let result = parse_llm_json(input).expect("should extract embedded JSON");
    assert_eq!(result["x"], 42);
}

#[test]
fn parse_embedded_array() {
    let input = "Results: [1, 2, 3] end.";
    let result = parse_llm_json(input).expect("should extract embedded array");
    assert_eq!(result.as_array().expect("array").len(), 3);
}

#[test]
fn parse_nested_braces() {
    let input = r#"Look: {"outer": {"inner": true}} done."#;
    let result = parse_llm_json(input).expect("should handle nested braces");
    assert_eq!(result["outer"]["inner"], true);
}

#[test]
fn parse_returns_none_for_garbage() {
    assert!(parse_llm_json("no json here at all").is_none());
}

#[test]
fn parse_returns_none_for_empty() {
    assert!(parse_llm_json("").is_none());
}

#[test]
fn parse_with_whitespace_padding() {
    let input = "   \n  {\"padded\": true}  \n  ";
    let result = parse_llm_json(input).expect("should handle whitespace");
    assert_eq!(result["padded"], true);
}

#[test]
fn parse_string_with_braces() {
    let input = r#"{"msg": "curly {braces} inside"}"#;
    let result = parse_llm_json(input).expect("should handle braces in strings");
    assert_eq!(result["msg"], "curly {braces} inside");
}

#[test]
fn parse_string_with_escaped_quotes() {
    let input = r#"{"msg": "he said \"hello\""}"#;
    let result = parse_llm_json(input).expect("should handle escaped quotes");
    assert_eq!(result["msg"], r#"he said "hello""#);
}

// -- retry_with_feedback tests ----------------------------------------------

#[test]
fn retry_succeeds_on_first_try() {
    let result = retry_with_feedback(|| Ok(42), 3, Duration::from_millis(1));
    assert_eq!(result.expect("should succeed"), 42);
}

#[test]
fn retry_succeeds_after_failures() {
    let mut count = 0u32;
    let result = retry_with_feedback(
        || {
            count += 1;
            if count < 3 {
                Err("nope".into())
            } else {
                Ok(count)
            }
        },
        3,
        Duration::from_millis(1),
    );
    assert_eq!(result.expect("should succeed on 3rd try"), 3);
}

#[test]
fn retry_exhausted() {
    let result: Result<(), _> =
        retry_with_feedback(|| Err("always fails".into()), 2, Duration::from_millis(1));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("retry exhausted"));
}

// -- isolate_prompt tests ---------------------------------------------------

#[test]
fn strips_xml_tags() {
    let input = "<thinking>internal</thinking>Hello!";
    assert_eq!(isolate_prompt(input), "internalHello!");
}

#[test]
fn strips_role_prefixes() {
    let input = "Assistant: Here is your answer.";
    assert_eq!(isolate_prompt(input), "Here is your answer.");
}

#[test]
fn strips_combined_noise() {
    let input = "Assistant: <thinking>hmm</thinking>\n\nActual output.";
    assert_eq!(isolate_prompt(input), "hmm\nActual output.");
}

#[test]
fn preserves_clean_text() {
    let input = "Just plain text.";
    assert_eq!(isolate_prompt(input), "Just plain text.");
}

#[test]
fn collapses_blank_lines() {
    let input = "line1\n\n\n\nline2";
    assert_eq!(isolate_prompt(input), "line1\nline2");
}

// -- read/write file with retry tests ---------------------------------------

#[test]
fn read_nonexistent_file_fails() {
    let result = read_file_with_retry(Path::new("/nonexistent/path"), 1, Duration::from_millis(1));
    assert!(result.is_err());
}

#[test]
fn write_and_read_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.txt");
    write_file_with_retry(&path, "hello", 1, Duration::from_millis(1))
        .expect("write should succeed");
    let content =
        read_file_with_retry(&path, 1, Duration::from_millis(1)).expect("read should succeed");
    assert_eq!(content, "hello");
}

#[test]
fn write_creates_parent_dirs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sub").join("dir").join("file.txt");
    write_file_with_retry(&path, "nested", 1, Duration::from_millis(1))
        .expect("should create parent dirs");
    assert!(path.exists());
}

// -- validate_json_schema tests ---------------------------------------------

#[test]
fn all_fields_present() {
    let data = json!({"name": "Alice", "age": 30});
    let missing = validate_json_schema(&data, &["name", "age"]);
    assert!(missing.is_empty());
}

#[test]
fn some_fields_missing() {
    let data = json!({"name": "Alice"});
    let missing = validate_json_schema(&data, &["name", "email"]);
    assert_eq!(missing, vec!["email"]);
}

#[test]
fn all_fields_missing() {
    let data = json!({});
    let missing = validate_json_schema(&data, &["a", "b"]);
    assert_eq!(missing, vec!["a", "b"]);
}

#[test]
fn non_object_returns_all_missing() {
    let data = json!([1, 2, 3]);
    let missing = validate_json_schema(&data, &["x"]);
    assert_eq!(missing, vec!["x"]);
}

#[test]
fn empty_required_fields() {
    let data = json!({"a": 1});
    let missing = validate_json_schema(&data, &[]);
    assert!(missing.is_empty());
}
