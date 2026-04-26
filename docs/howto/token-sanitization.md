# Token Sanitization Guide

Prevent token exposure in logs, errors, and debug output with automatic
sanitization.

!!! note "Rust Port"
    In amplihack-rs, token sanitization is implemented in the
    `amplihack-security` crate using compiled regex patterns. The Rust
    implementation provides the same detection coverage as the upstream
    Python `TokenSanitizer` with lower overhead.

## Quick Start

```python
# Upstream Python API (reference only)
from amplihack.utils.token_sanitizer import TokenSanitizer

sanitizer = TokenSanitizer()

error_msg = "Authentication failed with token: gho_abc123xyz"
safe_msg = sanitizer.sanitize(error_msg)
print(safe_msg)
# Output: "Authentication failed with token: [REDACTED-GITHUB-TOKEN]"
```

## What Gets Sanitized

TokenSanitizer detects and redacts these token types:

| Token Type | Pattern | Redaction Marker |
|---|---|---|
| GitHub tokens | `gho_*`, `ghp_*`, `ghs_*`, `ghu_*`, `ghr_*` | `[REDACTED-GITHUB-TOKEN]` |
| OpenAI keys | `sk-*`, `sk-proj-*` | `[REDACTED-OPENAI-KEY]` |
| Anthropic keys | `sk-ant-*` | `[REDACTED-ANTHROPIC-KEY]` |
| Bearer tokens | `Bearer <token>` | `[REDACTED-BEARER-TOKEN]` |
| JWT tokens | `eyJ*.eyJ*.*` | `[REDACTED-JWT-TOKEN]` |
| Azure keys | `azure-key-*` | `[REDACTED-AZURE-KEY]` |
| Azure connections | `DefaultEndpointsProtocol=...` | `[REDACTED-AZURE-CONNECTION]` |

## Common Use Cases

### Sanitizing API Errors

When API calls fail, error messages often contain authentication tokens:

```python
# Upstream Python API (reference only)
sanitizer = TokenSanitizer()

try:
    response = github_api.chat_completion(token="gho_abc123xyz")
except Exception as e:
    safe_error = sanitizer.sanitize(str(e))
    logger.error(f"API call failed: {safe_error}")
```

### Sanitizing Configuration Dumps

When debugging configuration, sanitize before printing:

```python
# Upstream Python API (reference only)
config = {
    "github_token": "gho_1234567890abcdefghij",
    "openai_key": "sk-proj-abc123xyz",
    "endpoint": "https://api.github.com"
}

sanitizer = TokenSanitizer()
safe_config = sanitizer.sanitize(config)
print(safe_config)
# Output: {'github_token': '[REDACTED-GITHUB-TOKEN]',
#          'openai_key': '[REDACTED-OPENAI-KEY]',
#          'endpoint': 'https://api.github.com'}
```

### Sanitizing Log Files

Process existing log files to remove tokens:

```python
# Upstream Python API (reference only)
from pathlib import Path

sanitizer = TokenSanitizer()
log_file = Path("debug.log")

content = log_file.read_text()
sanitized = sanitizer.sanitize(content)
log_file.write_text(sanitized)
```

## Integration Examples

### Logging Wrapper

Create a logging wrapper that auto-sanitizes:

```python
# Upstream Python API (reference only)
import logging

class SanitizingLogger:
    def __init__(self, name: str):
        self.logger = logging.getLogger(name)
        self.sanitizer = TokenSanitizer()

    def debug(self, msg: str, *args, **kwargs):
        self.logger.debug(self.sanitizer.sanitize(msg), *args, **kwargs)

    def error(self, msg: str, *args, **kwargs):
        self.logger.error(self.sanitizer.sanitize(msg), *args, **kwargs)

# Usage
logger = SanitizingLogger(__name__)
logger.debug(f"Token: {github_token}")  # Automatically sanitized
```

### Rust Integration

In amplihack-rs, token sanitization is automatic in the tracing pipeline:

```rust
// Token sanitization happens transparently in the tracing subscriber
// No manual calls needed in most cases
use tracing::info;

info!("Processing request with auth header");
// Any tokens in the span context are automatically redacted in log output
```

## Performance

TokenSanitizer is optimized for production use:

| Operation | Latency |
|---|---|
| Simple strings | < 1ms |
| Small dicts | < 1ms |
| 1000 strings | < 1 second total |
| Compiled regex | Patterns compiled once at initialization |

## Troubleshooting

### False Positives

If legitimate data is redacted:

- Check if your data resembles a token pattern
- Use allowlists for known-safe patterns
- Consider restructuring data to avoid token-like values

### Performance Issues

If sanitization is slow:

1. **Profile token density**: Check before sanitizing

    ```python
    if sanitizer.contains_token(text):
        text = sanitizer.sanitize(text)
    ```

2. **Reduce nested depth**: Very deep nesting (10+ levels) impacts performance
3. **Batch processing**: Process in chunks for huge datasets

## Best Practices

1. **Sanitize at boundaries**: Sanitize data when it crosses trust boundaries (logging, errors, API responses)
2. **Don't sanitize business logic**: Only sanitize for output/logging, not internal processing
3. **Use in exception handlers**: Always sanitize exceptions before displaying
4. **Test with real tokens**: Use actual token formats in tests (with pragma comments)
5. **Check before expensive operations**: Use `contains_token()` before deep sanitization

## Related Documentation

- [Security API Reference](../reference/security-api.md) — complete API documentation
- [Security Testing Guide](security-testing.md) — how to test security features
- [Security Recommendations](../reference/security-recommendations.md) — security best practices

---

**Remember**: TokenSanitizer protects against accidental token exposure. It is
not a replacement for proper secret management, secure storage, or encryption.
