# Security: Context Preservation Protection

**Type**: Explanation (Understanding-Oriented)

Comprehensive security enhancements in the context preservation system,
protecting against regex denial-of-service (ReDoS) attacks and input validation
vulnerabilities.

## Vulnerabilities Addressed

### Regex Denial-of-Service (ReDoS)

Unvalidated user input processed through regex operations can cause exponential
backtracking, leading to application hang or crash. All regex-heavy parsing
methods (`_parse_requirements`, `_parse_constraints`, `_parse_success_criteria`,
`_parse_target`, `get_latest_session_id`) now use timeout-protected wrappers.

### Input Size Attacks

Unlimited input size could cause memory exhaustion. Protection:

- Maximum input size: 50 KB
- Maximum line length: 1,000 characters
- Early validation before processing

### Input Injection

Malicious content in user input could be stored and executed. Protection:

- Unicode normalization (NFKC)
- Character whitelist filtering
- HTML escaping in output
- Content sanitization

## Security Architecture

### SecurityConfig

Centralized limits:

| Parameter          | Value   | Purpose                    |
| ------------------ | ------- | -------------------------- |
| `MAX_INPUT_SIZE`   | 50 KB   | Maximum input              |
| `MAX_LINE_LENGTH`  | 1,000   | Maximum line length        |
| `MAX_SENTENCES`    | 100     | Maximum sentences          |
| `MAX_BULLETS`      | 20      | Maximum bullet points      |
| `MAX_REQUIREMENTS` | 10      | Maximum requirements       |
| `MAX_CONSTRAINTS`  | 5       | Maximum constraints        |
| `MAX_CRITERIA`     | 5       | Maximum success criteria   |
| `REGEX_TIMEOUT`    | 1.0 s   | Regex operation timeout    |

### SecurityValidator

Safe wrappers for all regex operations:

- `validate_input_size()` — enforces size limits
- `sanitize_input()` — applies whitelist filtering
- `safe_regex_finditer()` — timeout-protected finditer
- `safe_regex_search()` — timeout-protected search
- `safe_regex_findall()` — timeout-protected findall
- `safe_split()` — timeout-protected split

## Protection Mechanisms

### Timeout Protection

SIGALRM-based timeouts (Unix/Linux/macOS) with graceful fallback for Windows.
Each regex operation has a 1-second maximum.

### Input Sanitization

Character whitelist approach: only alphanumerics, whitespace, punctuation, and
common symbols are allowed. Unicode is normalized via NFKC before filtering.

### Result Limiting

All operations cap the number of results returned, preventing memory exhaustion
from large result sets.

### Fail-Safe Error Handling

Operations fail securely with fallback responses. Security errors never expose
system internals.

```
# Example fail-safe pattern:
except (RegexTimeoutError, Exception):
    # Secure fallback without exposing error details
    requirements.append("[Requirements extraction failed - manual review needed]")
```

## Security Principles Applied

| Principle                  | Implementation                            |
| -------------------------- | ----------------------------------------- |
| **Defense in Depth**       | Input validation + sanitization + timeout + limits |
| **Least Privilege**        | Minimal allowed character set             |
| **Fail Secure**            | Default deny on validation failure        |
| **Input Validation**       | Server-side, whitelist over blacklist     |

## amplihack-rs Considerations

In the Rust port, equivalent protections are implemented using:

- `regex` crate with built-in backtracking limits (no ReDoS by default)
- Input size validation at deserialization boundaries
- `serde` field-level size constraints
- Rust's ownership model prevents many injection classes

The upstream Python protections documented here inform the Rust implementation's
threat model even where Rust provides stronger defaults.

## Related

- [Security Recommendations](../reference/security-recommendations.md) — operational security checklist
- [Security Audit: Copilot CLI Flags](../reference/security-audit-copilot-cli-flags.md) — review of flag isolation
