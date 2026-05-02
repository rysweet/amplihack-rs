// crates/amplihack-reflection/tests/security.rs
//
// TDD: failing tests for ContentSanitizer (port of
// amplifier-bundle/tools/amplihack/reflection/security.py).

use amplihack_reflection::security::ContentSanitizer;

#[test]
fn redacts_password_assignment() {
    let s = ContentSanitizer::new();
    let out = s.sanitize_content("password=hunter2 something else", 200);
    assert!(out.contains("[REDACTED]"));
    assert!(!out.contains("hunter2"));
}

#[test]
fn redacts_bearer_token() {
    let s = ContentSanitizer::new();
    let out = s.sanitize_content("Authorization: bearer=abcDEF123ghiJKL456mnop", 200);
    assert!(out.contains("[REDACTED]"));
    assert!(!out.contains("abcDEF123ghiJKL456mnop"));
}

#[test]
fn redacts_long_hex_credential() {
    let s = ContentSanitizer::new();
    let secret = "a".repeat(40);
    let out = s.sanitize_content(&format!("token={secret}"), 200);
    assert!(out.contains("[REDACTED]"));
}

#[test]
fn redacts_url_with_basic_auth() {
    let s = ContentSanitizer::new();
    let out = s.sanitize_content("https://user:supersecret@host/path", 200);
    assert!(!out.contains("supersecret"));
}

#[test]
fn redacts_env_variable_with_secret_name() {
    let s = ContentSanitizer::new();
    let out = s.sanitize_content("export $API_SECRET_KEY", 200);
    assert!(out.contains("[REDACTED]"));
}

#[test]
fn truncates_to_max_length() {
    let s = ContentSanitizer::new();
    let big = "x".repeat(10_000);
    let out = s.sanitize_content(&big, 100);
    assert!(
        out.len() <= 200,
        "expected truncation, got len {}",
        out.len()
    );
}

#[test]
fn benign_content_passes_through_untouched() {
    let s = ContentSanitizer::new();
    let benign = "this is a perfectly normal log line";
    let out = s.sanitize_content(benign, 200);
    assert_eq!(out, benign);
}

#[test]
fn filter_pattern_suggestion_removes_secrets() {
    let s = ContentSanitizer::new();
    let out = s.filter_pattern_suggestion("Try setting password=hunter2 in env");
    assert!(!out.contains("hunter2"));
}
