# Security Testing Guide

How to test security features in amplihack, including token sanitization, input
validation, and file permissions.

!!! note "Rust Port"
    In amplihack-rs, security tests use standard Rust testing with `cargo test`.
    The upstream Python test patterns translate directly to Rust `#[test]`
    functions. The testing pyramid ratios (60/30/10) remain the same.

## Testing Pyramid

Security tests follow the 60/30/10 testing pyramid:

- **60% Unit Tests**: Fast, isolated, heavily mocked
- **30% Integration Tests**: Multiple components, real scenarios
- **10% E2E Tests**: Complete workflows, minimal mocking

### Why This Distribution?

1. **Unit tests (60%)**: Catch bugs early, run in milliseconds
2. **Integration tests (30%)**: Verify component interactions
3. **E2E tests (10%)**: Validate complete user workflows

## Running Security Tests

### All Security Tests

```bash
# Rust tests
cargo test --package amplihack-security -- --nocapture

# With coverage (requires cargo-tarpaulin)
cargo tarpaulin --packages amplihack-security --out Html
```

### Upstream Python Tests (reference only)

```bash
# From project root
pytest tests/ -k "security or sanitiz" -v

# Run with coverage
pytest tests/ -k "security or sanitiz" \
  --cov=amplihack.tracing.token_sanitizer \
  --cov-report=term-missing
```

### Specific Test Categories

```bash
# Unit tests only (fast)
cargo test --package amplihack-security -- token_pattern
cargo test --package amplihack-security -- string_sanitization

# Integration tests
cargo test --package amplihack-security -- real_error_scenarios

# E2E tests
cargo test --package amplihack-security -- end_to_end
```

### Continuous Integration

Security tests run automatically on:

- Every commit to main branch
- Every pull request
- Pre-commit hooks (optional)

## Writing Security Tests

### Test Structure (Rust)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Unit tests (60%)
    #[test]
    fn test_github_token_detection() {
        let sanitizer = TokenSanitizer::new();
        assert!(sanitizer.contains_token("gho_1234567890abcdefghij"));
        assert!(!sanitizer.contains_token("github oauth token"));
        assert!(!sanitizer.contains_token("gho_short")); // Too short
    }

    // Integration tests (30%)
    #[test]
    fn test_sanitize_github_api_error() {
        let sanitizer = TokenSanitizer::new();
        let error_msg = r#"
            HTTP 401 Unauthorized
            Authorization: Bearer gho_1234567890abcdefghij
            Response: {"message": "Bad credentials"}
        "#;

        let result = sanitizer.sanitize(error_msg);
        assert!(!result.contains("gho_1234567890abcdefghij"));
        assert!(result.contains("[REDACTED-GITHUB-TOKEN]"));
        assert!(result.contains("Bad credentials"));
    }

    // E2E tests (10%)
    #[test]
    fn test_complete_sanitization_workflow() {
        // Test complete user journey
        let sanitizer = TokenSanitizer::new();
        let report = generate_error_report_with_tokens();
        let sanitized = sanitizer.sanitize(&report);
        assert!(!sanitizer.contains_token(&sanitized));
    }
}
```

### Test Structure (Upstream Python, reference only)

```python
"""Tests for [feature] security.

Testing pyramid:
- 60% Unit tests (fast, heavily mocked)
- 30% Integration tests (multiple components)
- 10% E2E tests (complete workflows)
"""

import pytest
from amplihack.tracing.token_sanitizer import TokenSanitizer

# Unit tests (60%)
class TestBasicFunctionality:
    def test_specific_behavior(self):
        sanitizer = TokenSanitizer()
        result = sanitizer.sanitize("test input")
        assert result == "expected output"

# Integration tests (30%)
class TestRealScenarios:
    def test_real_world_case(self):
        pass

# E2E tests (10%)
class TestCompleteWorkflows:
    def test_full_workflow(self):
        pass
```

## Test Coverage Requirements

| Category | Minimum Coverage |
|---|---|
| Token detection patterns | 100% |
| Sanitization logic | 95% |
| Edge cases | 90% |
| Integration scenarios | 85% |
| Overall security module | 90% |

## Common Test Patterns

### Boundary Testing

```rust
#[test]
fn test_token_boundaries() {
    let sanitizer = TokenSanitizer::new();

    // Minimum length tokens
    assert!(!sanitizer.contains_token("gho_12345"));      // Too short
    assert!(sanitizer.contains_token("gho_123456"));       // Minimum valid

    // Maximum length tokens
    let long_token = format!("gho_{}", "a".repeat(100));
    assert!(sanitizer.contains_token(&long_token));

    // Tokens at string boundaries
    assert!(sanitizer.contains_token("gho_123456"));       // Start
    assert!(sanitizer.contains_token("text gho_123456"));  // Middle
    assert!(sanitizer.contains_token("gho_123456 text"));  // End
}
```

### Multi-Token Testing

```rust
#[test]
fn test_multiple_tokens_in_one_string() {
    let sanitizer = TokenSanitizer::new();
    let input = "GitHub: gho_abc123xyz, OpenAI: sk-abc123xyz456";
    let result = sanitizer.sanitize(input);

    assert!(result.contains("[REDACTED-GITHUB-TOKEN]"));
    assert!(result.contains("[REDACTED-OPENAI-KEY]"));
    assert!(!result.contains("gho_abc123xyz"));
    assert!(!result.contains("sk-abc123xyz456"));
}
```

### Negative Testing

```rust
#[test]
fn test_no_false_positives() {
    let sanitizer = TokenSanitizer::new();

    // Should NOT be redacted
    assert!(!sanitizer.contains_token("ghost_writer"));
    assert!(!sanitizer.contains_token("skeleton_key"));
    assert!(!sanitizer.contains_token("just plain text"));
    assert!(!sanitizer.contains_token("gho_short"));
}
```

## Related Documentation

- [Token Sanitization Guide](token-sanitization.md) — usage guide
- [Security API Reference](../reference/security-api.md) — complete API docs
- [Security Recommendations](../reference/security-recommendations.md) — best practices
