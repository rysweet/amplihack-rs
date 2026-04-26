# Security API Reference

Complete reference for the token sanitizer module.

!!! note "Rust Port"
    In amplihack-rs, the token sanitizer is implemented in the
    `amplihack-security` crate. The Rust API mirrors the upstream Python
    `TokenSanitizer` class with idiomatic Rust patterns.

## TokenSanitizer

### Overview

TokenSanitizer detects and redacts sensitive tokens from strings and data
structures. Designed for production use with < 1ms performance for typical
operations.

**Philosophy**:

- Single responsibility: Token detection and sanitization
- Zero-BS: Fully functional with no stubs
- Performance-first: Compiled regex, minimal overhead

### Rust API

```rust
use amplihack_security::TokenSanitizer;

let sanitizer = TokenSanitizer::new();

// Check for tokens
assert!(sanitizer.contains_token("gho_abc123xyz"));

// Sanitize a string
let result = sanitizer.sanitize("Token: gho_abc123xyz");
assert_eq!(result, "Token: [REDACTED-GITHUB-TOKEN]");
```

### Upstream Python API (reference only)

```python
from amplihack.tracing.token_sanitizer import TokenSanitizer

sanitizer = TokenSanitizer()
```

---

### `new()` / `TokenSanitizer()`

Create a new TokenSanitizer instance with compiled regex patterns.

**Rust:**

```rust
let sanitizer = TokenSanitizer::new();
```

**Python (upstream):**

```python
sanitizer = TokenSanitizer()
```

**Thread Safety**: Yes — patterns are immutable after initialization.

**Performance**: O(1) — patterns compiled once.

---

### `contains_token`

Check if text contains any sensitive tokens.

**Rust:**

```rust
fn contains_token(&self, text: &str) -> bool
```

**Python (upstream):**

```python
def contains_token(self, text: str) -> bool
```

**Arguments**:

- `text`: Text to check for tokens

**Returns**: `true` if tokens detected, `false` otherwise

**Example:**

```rust
let sanitizer = TokenSanitizer::new();

assert!(sanitizer.contains_token("gho_abc123xyz"));     // true
assert!(!sanitizer.contains_token("no tokens here"));    // false
```

**Performance**: O(n) where n is text length

- Average: < 0.1ms for 1KB text
- Worst case: < 0.5ms for 10KB text

---

### `sanitize`

Sanitize tokens from a string. Returns a new string with tokens redacted.

**Rust:**

```rust
fn sanitize(&self, text: &str) -> String
```

**Python (upstream):**

```python
def sanitize(self, data: Any) -> Any
```

!!! note
    The Python version recursively processes strings, dicts, and lists.
    The Rust version operates on `&str` — use iterators for collections.

**Arguments**:

- `text`: Text to sanitize

**Returns**: New string with tokens redacted

**Examples:**

**String sanitization:**

```rust
let sanitizer = TokenSanitizer::new();

let result = sanitizer.sanitize("Token: gho_abc123xyz");
assert_eq!(result, "Token: [REDACTED-GITHUB-TOKEN]");
```

**Multiple tokens:**

```rust
let input = "GitHub: gho_abc123, OpenAI: sk-abc123456789";
let result = sanitizer.sanitize(input);
assert!(result.contains("[REDACTED-GITHUB-TOKEN]"));
assert!(result.contains("[REDACTED-OPENAI-KEY]"));
```

**Performance**:

- Strings: O(n) where n is string length
- Average: < 1ms for typical data

---

## Token Patterns

All patterns are compiled at initialization time and immutable thereafter.

### Supported Token Types

#### GitHub Tokens

**Prefixes**: `gho_`, `ghp_`, `ghs_`, `ghu_`, `ghr_`

**Pattern**: `gh[opsuhr]_[A-Za-z0-9]{6,100}`

**Replacement**: `[REDACTED-GITHUB-TOKEN]`

**Examples**:

| Input | Matched? |
|---|---|
| `gho_1234567890abcdefghij` | ✓ Yes |
| `ghp_1234567890abcdefghij` | ✓ Yes |
| `gho_short` | ✗ No (< 6 chars after prefix) |
| `ghost_writer` | ✗ No (not a valid prefix) |

#### OpenAI Keys

**Prefixes**: `sk-`, `sk-proj-`

**Pattern**: `sk-(?:proj-)?[A-Za-z0-9]{10,100}`

**Replacement**: `[REDACTED-OPENAI-KEY]`

#### Anthropic Keys

**Prefix**: `sk-ant-`

**Pattern**: `sk-ant-[A-Za-z0-9]{10,100}`

**Replacement**: `[REDACTED-ANTHROPIC-KEY]`

#### Bearer Tokens

**Pattern**: `Bearer\s+[A-Za-z0-9._~+/=-]{10,}`

**Replacement**: `[REDACTED-BEARER-TOKEN]`

#### JWT Tokens

**Pattern**: `eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+`

**Replacement**: `[REDACTED-JWT-TOKEN]`

#### Azure Keys

**Pattern**: `azure-key-[A-Za-z0-9]{10,}`

**Replacement**: `[REDACTED-AZURE-KEY]`

#### Azure Connection Strings

**Pattern**: `DefaultEndpointsProtocol=https?;[^\s]{10,}`

**Replacement**: `[REDACTED-AZURE-CONNECTION]`

---

## Performance Characteristics

### Benchmarks

| Operation | Input Size | Latency |
|---|---|---|
| `contains_token` | 100 bytes (no token) | 0.02ms |
| `contains_token` | 1 KB (with token) | 0.08ms |
| `sanitize` | 100 bytes (1 token) | 0.05ms |
| `sanitize` | 1 KB (3 tokens) | 0.15ms |
| `sanitize` | 10 KB (10 tokens) | 0.8ms |
| Init (`new()`) | — | 0.5ms |

### Memory Usage

- Per instance: ~2KB (compiled regex patterns)
- Per sanitization: O(n) temporary allocation for output string
- Patterns: Shared across all operations, immutable

### Thread Safety

TokenSanitizer is fully thread-safe:

- All methods are `&self` (read-only)
- No internal mutation after initialization
- Safe to share across threads via `Arc<TokenSanitizer>`

## Related Documentation

- [Token Sanitization Guide](../howto/token-sanitization.md) — usage guide
- [Security Testing Guide](../howto/security-testing.md) — testing patterns
- [Security Recommendations](security-recommendations.md) — best practices
- [Security Context Preservation](../concepts/security-context-preservation.md) — context security
