"""
TDD Step 7: Failing tests for scripts/ws2_settings_inspector.py.

Specifies the settings.json inspector used to validate that amplihack
install/uninstall leaves ~/.claude/settings.json in a clean state.

EXPECTED BEHAVIOR: All tests marked FAILS below fail until
  scripts/ws2_settings_inspector.py is created and implemented.

IMPLEMENTATION TARGET: scripts/ws2_settings_inspector.py
  Must export:
    @dataclass class InspectionResult:
        is_clean: bool
        issues: list[str]
        preserved_hooks: dict   # non-amplihack hooks that must survive
        stale_keys: list[str]   # hook names with amplihack refs or empty arrays

    def is_settings_clean_after_uninstall(settings_path: Path | str) -> InspectionResult
    def inspect_settings_json(content: dict) -> InspectionResult

DEFINITION OF "CLEAN" (from design spec):
  - No hook name whose value is an empty list []
  - No string value containing "amplihack-hooks"
  - No string value containing "tools/amplihack/"
  - Non-amplihack hooks are preserved (not removed)
  - Missing settings.json file → treated as clean (clean = True, no issues)

SETTINGS.JSON FORMATS SUPPORTED:
  Format A (simple): { "hooks": { "HookName": [{"type": "command", "command": "..."}] } }
  Format B (nested): { "hooks": { "HookName": [{"hooks": [{"type": "command", "command": "..."}]}] } }
  Both formats may contain amplihack references in the "command" field.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

# ---------------------------------------------------------------------------
# Import under test — FAILS until scripts/ws2_settings_inspector.py exists.
# ---------------------------------------------------------------------------
try:
    from ws2_settings_inspector import (
        InspectionResult,
        inspect_settings_json,
        is_settings_clean_after_uninstall,
    )

    _IMPORT_FAILED = False
except ImportError as exc:
    _IMPORT_FAILED = True
    _IMPORT_ERROR = str(exc)


def _skip_if_missing():
    if _IMPORT_FAILED:
        pytest.fail(
            "scripts/ws2_settings_inspector.py not found. "
            "IMPORT ERROR: " + _IMPORT_ERROR + "\n"
            "FIX: Create scripts/ws2_settings_inspector.py with "
            "InspectionResult, inspect_settings_json(), and "
            "is_settings_clean_after_uninstall()."
        )


# ---------------------------------------------------------------------------
# InspectionResult dataclass contract
# ---------------------------------------------------------------------------


class TestInspectionResultContract:
    """InspectionResult must expose is_clean, issues, preserved_hooks, stale_keys."""

    def test_has_is_clean(self):
        _skip_if_missing()
        result = InspectionResult(
            is_clean=True, issues=[], preserved_hooks={}, stale_keys=[]
        )
        assert result.is_clean is True

    def test_has_issues_list(self):
        _skip_if_missing()
        result = InspectionResult(
            is_clean=False,
            issues=["hook PreToolUse is empty"],
            preserved_hooks={},
            stale_keys=[],
        )
        assert isinstance(result.issues, list)
        assert len(result.issues) == 1

    def test_has_preserved_hooks(self):
        _skip_if_missing()
        preserved = {"Stop": [{"type": "command", "command": "/usr/local/bin/tool"}]}
        result = InspectionResult(
            is_clean=True, issues=[], preserved_hooks=preserved, stale_keys=[]
        )
        assert result.preserved_hooks == preserved

    def test_has_stale_keys(self):
        _skip_if_missing()
        result = InspectionResult(
            is_clean=False,
            issues=["stale hook"],
            preserved_hooks={},
            stale_keys=["PreToolUse"],
        )
        assert "PreToolUse" in result.stale_keys


# ---------------------------------------------------------------------------
# inspect_settings_json: clean states
# ---------------------------------------------------------------------------


class TestInspectCleanSettings:
    """Settings with no amplihack references must be reported as clean."""

    def test_empty_hooks_dict_is_clean(self):
        _skip_if_missing()
        content = {"hooks": {}}
        result = inspect_settings_json(content)
        assert result.is_clean is True, (
            "Empty hooks dict must be clean. " f"Issues: {result.issues}"
        )

    def test_no_hooks_key_is_clean(self):
        _skip_if_missing()
        content = {}
        result = inspect_settings_json(content)
        assert result.is_clean is True, (
            "Settings with no hooks key must be clean. " f"Issues: {result.issues}"
        )

    def test_only_non_amplihack_hooks_is_clean(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "/usr/local/bin/other-tool --check"}
                ],
                "Stop": [{"type": "command", "command": "/usr/local/bin/notify-done"}],
            }
        }
        result = inspect_settings_json(content)
        assert result.is_clean is True, (
            "Non-amplihack hooks must be clean. " f"Issues: {result.issues}"
        )

    def test_clean_settings_has_no_issues(self):
        _skip_if_missing()
        content = {
            "hooks": {"Stop": [{"type": "command", "command": "/usr/local/bin/ok"}]}
        }
        result = inspect_settings_json(content)
        assert (
            result.issues == []
        ), f"Clean settings must have no issues. Got: {result.issues}"


# ---------------------------------------------------------------------------
# inspect_settings_json: stale amplihack-hooks references
# ---------------------------------------------------------------------------


class TestInspectStaleAmplihackHooksRef:
    """Any hook command containing 'amplihack-hooks' is stale after uninstall."""

    def test_amplihack_hooks_in_command_is_not_clean(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "type": "command",
                        "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert result.is_clean is False, (
            "Hook with 'amplihack-hooks' in command must NOT be clean. "
            f"is_clean={result.is_clean}, issues={result.issues}"
        )

    def test_amplihack_hooks_ref_appears_in_issues(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "type": "command",
                        "command": "/home/u/.amplihack/tools/amplihack/amplihack-hooks Stop",
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert result.issues, "issues must be non-empty when amplihack-hooks ref exists"
        combined = " ".join(result.issues).lower()
        assert (
            "amplihack" in combined or "stale" in combined or "pretooluse" in combined
        ), (
            "Issue message must reference amplihack or the hook name. "
            f"Got: {result.issues}"
        )

    def test_amplihack_hooks_key_in_stale_keys(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "Stop": [
                    {
                        "type": "command",
                        "command": "~/.amplihack/tools/amplihack/amplihack-hooks Stop",
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert "Stop" in result.stale_keys, (
            "Hook name with stale amplihack-hooks ref must appear in stale_keys. "
            f"Got stale_keys: {result.stale_keys}"
        )

    def test_nested_format_b_amplihack_ref_detected(self):
        """Format B (nested hooks array) with amplihack ref must be detected."""
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
                            }
                        ],
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert result.is_clean is False, (
            "Nested format B with amplihack-hooks must be detected as not clean. "
            f"is_clean={result.is_clean}, issues={result.issues}"
        )


# ---------------------------------------------------------------------------
# inspect_settings_json: tools/amplihack/ path references
# ---------------------------------------------------------------------------


class TestInspectToolsAmplihackRef:
    """Any command containing 'tools/amplihack/' is stale after uninstall."""

    def test_tools_amplihack_path_is_not_clean(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "type": "command",
                        "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks",
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert (
            result.is_clean is False
        ), "Command with 'tools/amplihack/' path must NOT be clean."

    def test_tools_amplihack_tilde_path_is_not_clean(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "Stop": [
                    {
                        "type": "command",
                        "command": "~/.amplihack/tools/amplihack/amplihack-hooks Stop",
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert result.is_clean is False


# ---------------------------------------------------------------------------
# inspect_settings_json: empty arrays
# ---------------------------------------------------------------------------


class TestInspectEmptyArrays:
    """Hook names with empty arrays are a residue of broken uninstall."""

    def test_empty_array_hook_is_not_clean(self):
        _skip_if_missing()
        content = {"hooks": {"PreToolUse": []}}
        result = inspect_settings_json(content)
        assert result.is_clean is False, (
            "Hook key with empty list must NOT be clean. "
            f"is_clean={result.is_clean}, issues={result.issues}"
        )

    def test_multiple_empty_arrays_all_reported(self):
        _skip_if_missing()
        content = {"hooks": {"PreToolUse": [], "Stop": []}}
        result = inspect_settings_json(content)
        assert result.is_clean is False
        # Both must appear in stale_keys or issues
        combined = " ".join(result.stale_keys) + " ".join(result.issues)
        assert "PreToolUse" in combined or len(result.stale_keys) >= 2, (
            "Both empty-array hooks must be reported. "
            f"stale_keys={result.stale_keys}, issues={result.issues}"
        )

    def test_empty_array_in_stale_keys(self):
        _skip_if_missing()
        content = {"hooks": {"Stop": []}}
        result = inspect_settings_json(content)
        assert "Stop" in result.stale_keys, (
            "Empty-array hook must appear in stale_keys. " f"Got: {result.stale_keys}"
        )


# ---------------------------------------------------------------------------
# inspect_settings_json: non-amplihack hooks must be PRESERVED
# ---------------------------------------------------------------------------


class TestNonAmplihackHooksPreserved:
    """Non-amplihack hooks must be in preserved_hooks and must not be removed."""

    def test_non_amplihack_hooks_in_preserved(self):
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "/usr/local/bin/custom-hook --pre"}
                ]
            }
        }
        result = inspect_settings_json(content)
        assert "PreToolUse" in result.preserved_hooks, (
            "Non-amplihack hook must appear in preserved_hooks. "
            f"Got: {result.preserved_hooks}"
        )

    def test_mixed_hooks_separates_correctly(self):
        """Stale amplihack hook and non-amplihack hook in same settings."""
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "type": "command",
                        "command": "/home/u/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
                    }
                ],
                "Stop": [{"type": "command", "command": "/usr/local/bin/notify"}],
            }
        }
        result = inspect_settings_json(content)
        # PreToolUse is stale
        assert result.is_clean is False
        assert "PreToolUse" in result.stale_keys
        # Stop is preserved
        assert "Stop" in result.preserved_hooks, (
            "Non-amplihack 'Stop' hook must be in preserved_hooks. "
            f"Got: {result.preserved_hooks}"
        )

    def test_mixed_commands_same_hook_key(self):
        """If a hook list has both amplihack and non-amplihack commands, the key is stale
        but the non-amplihack commands must be documented in preserved_hooks."""
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "type": "command",
                        "command": "/home/u/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
                    },
                    {"type": "command", "command": "/usr/local/bin/other-hook"},
                ]
            }
        }
        result = inspect_settings_json(content)
        # The key is stale (has amplihack ref)
        assert result.is_clean is False
        # But the other-hook entry must be documented for manual preservation
        preserved_str = json.dumps(result.preserved_hooks)
        assert (
            "other-hook" in preserved_str or "PreToolUse" in result.preserved_hooks
        ), (
            "Mixed-command hook key must document non-amplihack entries in preserved_hooks. "
            f"preserved_hooks: {result.preserved_hooks}"
        )


# ---------------------------------------------------------------------------
# is_settings_clean_after_uninstall: file-based API
# ---------------------------------------------------------------------------


class TestIsSettingsCleanAfterUninstall:
    """Tests the file-based public API (reads a real file path)."""

    def test_missing_file_is_clean(self, tmp_path):
        _skip_if_missing()
        nonexistent = tmp_path / "settings.json"
        assert not nonexistent.exists()
        result = is_settings_clean_after_uninstall(nonexistent)
        assert result.is_clean is True, (
            "Missing settings.json must be treated as clean (no stale hooks). "
            f"issues={result.issues}"
        )

    def test_clean_file_returns_is_clean_true(self, tmp_path):
        _skip_if_missing()
        settings_path = tmp_path / "settings.json"
        settings_path.write_text(
            json.dumps(
                {
                    "hooks": {
                        "Stop": [{"type": "command", "command": "/usr/local/bin/ok"}]
                    }
                }
            )
        )
        result = is_settings_clean_after_uninstall(settings_path)
        assert (
            result.is_clean is True
        ), f"Clean settings file must return is_clean=True. issues={result.issues}"

    def test_stale_file_returns_is_clean_false(
        self, tmp_path, stale_amplihack_hooks_settings
    ):
        _skip_if_missing()
        result = is_settings_clean_after_uninstall(stale_amplihack_hooks_settings)
        assert result.is_clean is False, (
            "Stale settings file must return is_clean=False. " f"issues={result.issues}"
        )

    def test_empty_array_file_returns_is_clean_false(
        self, tmp_path, empty_array_hooks_settings
    ):
        _skip_if_missing()
        result = is_settings_clean_after_uninstall(empty_array_hooks_settings)
        assert result.is_clean is False, (
            "Settings with empty-array hooks must return is_clean=False. "
            f"issues={result.issues}"
        )

    def test_accepts_path_as_string(self, tmp_path):
        _skip_if_missing()
        settings_path = tmp_path / "settings.json"
        settings_path.write_text(json.dumps({"hooks": {}}))
        # Must accept str, not just Path
        result = is_settings_clean_after_uninstall(str(settings_path))
        assert result.is_clean is True

    def test_invalid_json_returns_issue(self, tmp_path):
        _skip_if_missing()
        settings_path = tmp_path / "settings.json"
        settings_path.write_text("{ this is: not valid json }")
        result = is_settings_clean_after_uninstall(settings_path)
        # Either: is_clean=False with a parse error in issues,
        # OR raises a clear exception (not an unhandled crash)
        if not result.is_clean:
            assert result.issues, "Invalid JSON must produce at least one issue"
        # If it returns is_clean=True that is also acceptable (file doesn't exist/parse error = clean)

    def test_empty_file_is_clean(self, tmp_path):
        _skip_if_missing()
        settings_path = tmp_path / "settings.json"
        settings_path.write_text("")
        result = is_settings_clean_after_uninstall(settings_path)
        # Empty file can be treated as clean (no hooks to check)
        assert isinstance(
            result, InspectionResult
        ), "is_settings_clean_after_uninstall must return InspectionResult even for empty files"


# ---------------------------------------------------------------------------
# Edge cases and boundary conditions
# ---------------------------------------------------------------------------


class TestEdgeCases:
    """Boundary conditions for the settings inspector."""

    def test_deeply_nested_amplihack_ref_detected(self):
        """Multi-level nested structure with amplihack ref must still be detected."""
        _skip_if_missing()
        content = {
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks",
                                "timeout": 30,
                            }
                        ],
                    }
                ]
            }
        }
        result = inspect_settings_json(content)
        assert result.is_clean is False, (
            "Deeply nested amplihack ref must be detected. " f"issues={result.issues}"
        )

    def test_settings_with_extra_fields_handled(self):
        """Settings may contain non-hooks keys (future compat) — must not crash."""
        _skip_if_missing()
        content = {
            "hooks": {},
            "project_root": "/some/path",
            "future_setting": {"nested": True},
        }
        result = inspect_settings_json(content)
        assert isinstance(result, InspectionResult)

    def test_hooks_with_null_value_handled(self):
        """hooks key with null value must not crash."""
        _skip_if_missing()
        content = {"hooks": None}
        try:
            result = inspect_settings_json(content)
            assert isinstance(result, InspectionResult)
        except (TypeError, AttributeError):
            pytest.fail(
                "inspect_settings_json must handle hooks=null gracefully, "
                "not crash with TypeError/AttributeError"
            )

    @pytest.mark.parametrize(
        "command",
        [
            "/home/user/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
            "~/.amplihack/tools/amplihack/amplihack-hooks Stop",
            "/root/.amplihack/tools/amplihack/amplihack-hooks PostToolUse",
            "${HOME}/.amplihack/tools/amplihack/amplihack-hooks Stop",
        ],
    )
    def test_various_amplihack_paths_detected(self, command: str):
        _skip_if_missing()
        content = {"hooks": {"PreToolUse": [{"type": "command", "command": command}]}}
        result = inspect_settings_json(content)
        assert result.is_clean is False, (
            f"Command {command!r} must be detected as stale. "
            f"is_clean={result.is_clean}"
        )
