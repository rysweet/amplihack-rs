"""
TDD Step 7: Failing tests for scripts/shared/output_sanitizer.py.

These tests specify the contract for the output sanitizer used to redact
secrets from captured subprocess output before writing to PR comments or logs.

EXPECTED BEHAVIOR: All tests in this module FAIL until
  scripts/shared/output_sanitizer.py is created with the correct implementation.

IMPLEMENTATION TARGET: scripts/shared/output_sanitizer.py
  Must export: sanitize(text: str) -> str
  Must export: sanitize_dict(d: dict) -> dict
  Must export: REDACTED_PLACEHOLDER: str  (the replacement token)

SECURITY REQUIREMENT: The sanitizer must redact:
  - Anthropic API keys:   sk-ant-...  and sk-...
  - GitHub PATs:          ghp_..., ghs_..., github_pat_...
  - Bearer tokens:        "Bearer <token>"
  - password= patterns:   password=<value>  (in URLs, configs, etc.)
  - Azure tokens/keys:    common SAS token patterns
"""

from __future__ import annotations

import pytest

# ---------------------------------------------------------------------------
# Import under test — FAILS until scripts/shared/output_sanitizer.py exists.
# ---------------------------------------------------------------------------
try:
    from output_sanitizer import REDACTED_PLACEHOLDER, sanitize, sanitize_dict

    _IMPORT_FAILED = False
except ImportError as exc:
    _IMPORT_FAILED = True
    _IMPORT_ERROR = str(exc)


def _skip_if_missing():
    if _IMPORT_FAILED:
        pytest.fail(
            "scripts/shared/output_sanitizer.py not found. "
            "IMPORT ERROR: " + _IMPORT_ERROR + "\n"
            "FIX: Create scripts/shared/output_sanitizer.py with "
            "sanitize(text), sanitize_dict(d), and REDACTED_PLACEHOLDER."
        )


# ---------------------------------------------------------------------------
# Module-level contract tests
# ---------------------------------------------------------------------------


class TestModuleContract:
    """Verify the module exports the expected public API."""

    def test_sanitize_function_exists(self):
        _skip_if_missing()
        assert callable(sanitize), "sanitize must be a callable"

    def test_sanitize_dict_function_exists(self):
        _skip_if_missing()
        assert callable(sanitize_dict), "sanitize_dict must be a callable"

    def test_redacted_placeholder_is_string(self):
        _skip_if_missing()
        assert isinstance(REDACTED_PLACEHOLDER, str), (
            "REDACTED_PLACEHOLDER must be a str. " f"Got: {type(REDACTED_PLACEHOLDER)}"
        )

    def test_redacted_placeholder_is_nonempty(self):
        _skip_if_missing()
        assert (
            REDACTED_PLACEHOLDER.strip()
        ), "REDACTED_PLACEHOLDER must not be empty or whitespace-only"


# ---------------------------------------------------------------------------
# Happy-path: clean text is returned unchanged
# ---------------------------------------------------------------------------


class TestCleanTextPassthrough:
    """Text with no secrets must be returned unchanged."""

    def test_plain_text_unchanged(self):
        _skip_if_missing()
        text = "cargo test --workspace --locked\nAll tests passed."
        assert sanitize(text) == text

    def test_empty_string_unchanged(self):
        _skip_if_missing()
        assert sanitize("") == ""

    def test_none_returns_empty_or_none(self):
        """sanitize(None) must not raise — returns empty string or None."""
        _skip_if_missing()
        result = sanitize(None)  # type: ignore[arg-type]
        assert result is None or result == "", (
            "sanitize(None) must return None or '' without raising. " f"Got: {result!r}"
        )

    def test_multiline_clean_text_unchanged(self):
        _skip_if_missing()
        text = "line 1\nline 2\nBuild succeeded in 3.5s\n"
        assert sanitize(text) == text


# ---------------------------------------------------------------------------
# Anthropic API key patterns
# ---------------------------------------------------------------------------


class TestAnthropicApiKeyRedaction:
    """sk-ant-* and sk-* patterns must be redacted."""

    def test_sk_ant_key_is_redacted(self):
        _skip_if_missing()
        text = "ANTHROPIC_API_KEY=sk-ant-api03-abc123def456xyz789"
        result = sanitize(text)
        assert (
            "sk-ant-api03-abc123def456xyz789" not in result
        ), f"Anthropic API key must be redacted. Got: {result!r}"
        assert (
            REDACTED_PLACEHOLDER in result
        ), f"Redacted placeholder must appear in output. Got: {result!r}"

    def test_sk_key_in_export_statement(self):
        _skip_if_missing()
        text = "export ANTHROPIC_API_KEY=sk-ant-abc123-longkeyvalue"
        result = sanitize(text)
        assert "sk-ant-abc123-longkeyvalue" not in result
        assert REDACTED_PLACEHOLDER in result

    def test_sk_key_mid_line_is_redacted(self):
        _skip_if_missing()
        text = "Setting key to sk-ant-api03-realkey123 for test"
        result = sanitize(text)
        assert "sk-ant-api03-realkey123" not in result

    def test_anthropic_key_multiline_context(self):
        _skip_if_missing()
        text = (
            "Starting test run\n"
            "ANTHROPIC_API_KEY=sk-ant-secret999 loaded\n"
            "Running cargo test\n"
        )
        result = sanitize(text)
        assert "sk-ant-secret999" not in result
        # Non-secret lines are preserved
        assert "Starting test run" in result
        assert "Running cargo test" in result


# ---------------------------------------------------------------------------
# GitHub PAT patterns
# ---------------------------------------------------------------------------


class TestGitHubPatRedaction:
    """ghp_*, ghs_*, github_pat_* patterns must be redacted."""

    def test_ghp_token_redacted(self):
        _skip_if_missing()
        text = "GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz123456"
        result = sanitize(text)
        assert "ghp_abcdefghijklmnopqrstuvwxyz123456" not in result
        assert REDACTED_PLACEHOLDER in result

    def test_ghs_token_redacted(self):
        _skip_if_missing()
        text = "Authorization: token ghs_abc123xyz456"
        result = sanitize(text)
        assert "ghs_abc123xyz456" not in result
        assert REDACTED_PLACEHOLDER in result

    def test_github_pat_token_redacted(self):
        _skip_if_missing()
        text = "GITHUB_TOKEN=github_pat_11ABCDEF00abcdefgh_longtoken_here"
        result = sanitize(text)
        assert "github_pat_11ABCDEF00abcdefgh_longtoken_here" not in result
        assert REDACTED_PLACEHOLDER in result


# ---------------------------------------------------------------------------
# Bearer token patterns
# ---------------------------------------------------------------------------


class TestBearerTokenRedaction:
    """'Bearer <token>' patterns must be redacted."""

    def test_bearer_token_in_header_redacted(self):
        _skip_if_missing()
        text = "Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.payload.sig"
        result = sanitize(text)
        assert "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.payload.sig" not in result
        assert REDACTED_PLACEHOLDER in result

    def test_bearer_keyword_preserved_when_no_token(self):
        """The word 'Bearer' alone (without a following token) must not be redacted."""
        _skip_if_missing()
        text = "HTTP auth type is Bearer (JSON Web Token)"
        result = sanitize(text)
        # A bare "Bearer" with no token-like value should not trigger redaction
        # (or if it does, that's acceptable — but it must not destroy the whole line)
        assert "HTTP auth type is" in result

    def test_bearer_curl_command_redacted(self):
        _skip_if_missing()
        text = 'curl -H "Authorization: Bearer mysupersecrettoken123" https://api.example.com'
        result = sanitize(text)
        assert "mysupersecrettoken123" not in result


# ---------------------------------------------------------------------------
# password= patterns
# ---------------------------------------------------------------------------


class TestPasswordPatternRedaction:
    """password=<value> patterns must be redacted."""

    def test_password_in_url_redacted(self):
        _skip_if_missing()
        text = "Connecting to postgresql://user:password=supersecret@host:5432/db"
        result = sanitize(text)
        assert "supersecret" not in result

    def test_password_equals_redacted(self):
        _skip_if_missing()
        text = "DB_PASSWORD=mysecretpassword123 cargo test"
        result = sanitize(text)
        assert "mysecretpassword123" not in result
        assert REDACTED_PLACEHOLDER in result

    def test_password_key_in_config_line(self):
        _skip_if_missing()
        text = "  password=hunter2"
        result = sanitize(text)
        assert "hunter2" not in result


# ---------------------------------------------------------------------------
# sanitize_dict: redacts values in a dict
# ---------------------------------------------------------------------------


class TestSanitizeDict:
    """sanitize_dict must recursively sanitize string values."""

    def test_sanitize_dict_redacts_api_key_value(self):
        _skip_if_missing()
        d = {"stdout": "key=sk-ant-secret999", "exit_code": 0}
        result = sanitize_dict(d)
        assert "sk-ant-secret999" not in str(result)
        assert result["exit_code"] == 0  # non-string values preserved

    def test_sanitize_dict_handles_nested(self):
        _skip_if_missing()
        d = {
            "run1": {"stdout": "ok", "stderr": "token=ghp_abc123"},
            "run2": {"stdout": "ok", "stderr": ""},
        }
        result = sanitize_dict(d)
        assert "ghp_abc123" not in str(result)
        assert result["run2"]["stdout"] == "ok"

    def test_sanitize_dict_does_not_mutate_original(self):
        _skip_if_missing()
        original = {"env": "GH_TOKEN=ghp_secret123"}
        _ = sanitize_dict(original)
        assert (
            "ghp_secret123" in original["env"]
        ), "sanitize_dict must return a new dict, not mutate the original"

    def test_sanitize_dict_preserves_non_string_values(self):
        _skip_if_missing()
        d = {"exit_code": 0, "success": True, "duration_ms": 1234}
        result = sanitize_dict(d)
        assert result == d
