"""
Output sanitizer: redacts secrets from captured subprocess output.

Exported API:
  sanitize(text: str) -> str
  sanitize_dict(d: dict) -> dict
  REDACTED_PLACEHOLDER: str
"""

from __future__ import annotations

import re

REDACTED_PLACEHOLDER = "[REDACTED]"

# Ordered list of (pattern, replacement) tuples.
# Patterns are applied in order; more specific patterns come first.
_PATTERNS: list[tuple[re.Pattern, str]] = [
    # Anthropic API keys: sk-ant-... variants
    (re.compile(r"sk-ant-[A-Za-z0-9_\-]+"), REDACTED_PLACEHOLDER),
    # Generic sk- keys (must come after sk-ant- to avoid double-match issues,
    # but since sk-ant- is more specific and applied first it's fine)
    (re.compile(r"sk-[A-Za-z0-9_\-]{20,}"), REDACTED_PLACEHOLDER),
    # GitHub fine-grained PATs: github_pat_...
    (re.compile(r"github_pat_[A-Za-z0-9_]+"), REDACTED_PLACEHOLDER),
    # GitHub app installation tokens: ghs_...
    (re.compile(r"ghs_[A-Za-z0-9_]+"), REDACTED_PLACEHOLDER),
    # GitHub classic PATs: ghp_...
    (re.compile(r"ghp_[A-Za-z0-9_]+"), REDACTED_PLACEHOLDER),
    # Bearer tokens: "Bearer <token>" — token is a non-whitespace sequence
    # Require the token to look like a real token (≥8 chars, no plain English)
    (
        re.compile(r"Bearer\s+([A-Za-z0-9\-_\.]+[A-Za-z0-9\-_\.]{7,})"),
        "Bearer " + REDACTED_PLACEHOLDER,
    ),
    # password= patterns (URL or config form)
    (re.compile(r"(?i)password=\S+"), "password=" + REDACTED_PLACEHOLDER),
    # DB_PASSWORD= or similar env var patterns
    (
        re.compile(r"(?i)[A-Z_]*PASSWORD[A-Z_]*=\S+"),
        lambda m: m.group(0).split("=")[0] + "=" + REDACTED_PLACEHOLDER,
    ),
]

# Fast pre-check: single regex scan to detect any string that might contain a
# secret pattern.  Most subprocess output (build logs, test results, etc.) will
# NOT match, allowing us to skip all eight pattern passes entirely.
# Conservative: uses broad terms so there are no false negatives.
_QUICK_CHECK = re.compile(r"sk-|github_pat_|ghs_|ghp_|Bearer\s|password", re.IGNORECASE)


def sanitize(text: str | None) -> str | None:
    """Redact secrets from a string.

    Args:
        text: The string to sanitize. May be None.

    Returns:
        The sanitized string with secrets replaced by REDACTED_PLACEHOLDER.
        If text is None, returns None.
        If text is empty, returns empty string.
    """
    if text is None:
        return None
    if not text:
        return text

    # Fast path: skip all regex passes when no secret-like token is present.
    # Avoids 8 compiled-regex sweeps on the common case of clean output.
    if not _QUICK_CHECK.search(text):
        return text

    result = text
    for pattern, replacement in _PATTERNS:
        result = pattern.sub(replacement, result)
    return result


def sanitize_dict(d: dict) -> dict:
    """Recursively sanitize all string values in a dict.

    Non-mutating: returns a new dict. The original is not modified.

    Args:
        d: The dict to sanitize.

    Returns:
        A new dict with all string values sanitized.
    """
    result = {}
    for key, value in d.items():
        if isinstance(value, str):
            result[key] = sanitize(value)
        elif isinstance(value, dict):
            result[key] = sanitize_dict(value)
        else:
            result[key] = value
    return result
