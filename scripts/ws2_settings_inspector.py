"""
WS2 Settings Inspector.

Inspects ~/.claude/settings.json to verify that amplihack install/uninstall
leaves the file in a clean state.

Definition of "CLEAN":
  - No hook key whose value is an empty list []
  - No string value (at any depth) containing "amplihack-hooks"
  - No string value (at any depth) containing "tools/amplihack/"
  - Non-amplihack hooks are preserved (documented in preserved_hooks)
  - Missing settings.json → clean (no hooks to check)

Exported API:
  @dataclass InspectionResult: is_clean, issues, preserved_hooks, stale_keys
  def inspect_settings_json(content: dict) -> InspectionResult
  def inspect_settings_string(text: str) -> InspectionResult
  def is_settings_clean_after_uninstall(settings_path: Path | str) -> InspectionResult
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_STALE_MARKERS = ("amplihack-hooks", "tools/amplihack/")


@dataclass
class InspectionResult:
    """Result of inspecting a settings.json for stale amplihack references."""

    is_clean: bool
    issues: list
    preserved_hooks: dict
    stale_keys: list


def _command_is_stale(command: str) -> bool:
    """Return True if a command string contains any stale amplihack marker."""
    for marker in _STALE_MARKERS:
        if marker in command:
            return True
    return False


def _extract_commands_from_hook_entry(entry: Any) -> list[str]:
    """Extract all command strings from a hook entry (handles Format A and B).

    Format A: {"type": "command", "command": "..."}
    Format B: {"matcher": "...", "hooks": [{"type": "command", "command": "..."}]}
    """
    commands = []
    if not isinstance(entry, dict):
        return commands

    # Format A: direct command field
    if "command" in entry and isinstance(entry["command"], str):
        commands.append(entry["command"])

    # Format B: nested hooks array
    if "hooks" in entry and isinstance(entry["hooks"], list):
        for nested in entry["hooks"]:
            commands.extend(_extract_commands_from_hook_entry(nested))

    return commands


def inspect_settings_json(content: dict) -> InspectionResult:
    """Inspect parsed settings.json content for stale amplihack references.

    Args:
        content: Parsed settings.json as a dict.

    Returns:
        InspectionResult describing the clean/stale state.
    """
    issues: list[str] = []
    preserved_hooks: dict = {}
    stale_keys: list[str] = []

    hooks = content.get("hooks") if isinstance(content, dict) else None

    if not hooks or not isinstance(hooks, dict):
        # No hooks key, null hooks, or empty hooks dict → clean
        return InspectionResult(
            is_clean=True,
            issues=[],
            preserved_hooks={},
            stale_keys=[],
        )

    for hook_name, hook_entries in hooks.items():
        # Empty array check
        if isinstance(hook_entries, list) and not hook_entries:
            issues.append(
                f"Hook '{hook_name}' has an empty array (stale residue of broken uninstall)"
            )
            stale_keys.append(hook_name)
            continue

        if not isinstance(hook_entries, list):
            # Skip non-list values without marking as stale
            continue

        # Check each entry for stale commands
        hook_is_stale = False
        non_amplihack_entries = []

        for entry in hook_entries:
            commands = _extract_commands_from_hook_entry(entry)
            entry_is_stale = any(_command_is_stale(cmd) for cmd in commands)

            if entry_is_stale:
                hook_is_stale = True
                issues.append(
                    f"Hook '{hook_name}' contains stale amplihack reference: "
                    + "; ".join(cmd for cmd in commands if _command_is_stale(cmd))
                )
            else:
                non_amplihack_entries.append(entry)

        if hook_is_stale:
            stale_keys.append(hook_name)
            # Document non-amplihack entries for manual preservation
            if non_amplihack_entries:
                preserved_hooks[hook_name] = non_amplihack_entries
        else:
            # Entire hook is clean — record it as preserved
            preserved_hooks[hook_name] = hook_entries

    is_clean = len(issues) == 0

    return InspectionResult(
        is_clean=is_clean,
        issues=issues,
        preserved_hooks=preserved_hooks,
        stale_keys=stale_keys,
    )


def inspect_settings_string(text: str) -> InspectionResult:
    """Inspect raw settings.json text for stale amplihack references.

    Parses *text* as JSON and delegates to :func:`inspect_settings_json`.
    Invalid JSON → reported as an issue (is_clean=False).
    Empty string → treated as clean.

    Args:
        text: The raw settings.json content as a string.

    Returns:
        InspectionResult describing the clean/stale state.
    """
    raw = text.strip()
    if not raw:
        return InspectionResult(
            is_clean=True, issues=[], preserved_hooks={}, stale_keys=[]
        )

    try:
        content = json.loads(raw)
    except json.JSONDecodeError as exc:
        return InspectionResult(
            is_clean=False,
            issues=[f"Failed to parse settings.json: {exc}"],
            preserved_hooks={},
            stale_keys=[],
        )

    if not isinstance(content, dict):
        return InspectionResult(
            is_clean=False,
            issues=["settings.json top-level value must be an object (dict)"],
            preserved_hooks={},
            stale_keys=[],
        )

    return inspect_settings_json(content)


def is_settings_clean_after_uninstall(settings_path: Path | str) -> InspectionResult:
    """Read a settings.json file and inspect it for stale amplihack references.

    Missing file → treated as clean (no hooks to check).
    Invalid JSON → reported as an issue (is_clean=False).
    Empty file → treated as clean.

    Args:
        settings_path: Path to the settings.json file (Path or str).

    Returns:
        InspectionResult describing the clean/stale state.
    """
    path = Path(settings_path)

    if not path.exists():
        return InspectionResult(
            is_clean=True, issues=[], preserved_hooks={}, stale_keys=[]
        )

    return inspect_settings_string(path.read_text())
