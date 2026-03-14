"""
Pytest configuration and shared fixtures for unit tests.

Path setup: inserts both the workspace root and scripts/ directory into sys.path
so tests can import from scripts/ without package installation.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Path setup — runs once at collection time
# ---------------------------------------------------------------------------
REPO_ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPTS_DIR = REPO_ROOT / "scripts"
SCRIPTS_SHARED_DIR = SCRIPTS_DIR / "shared"

for p in [str(REPO_ROOT), str(SCRIPTS_DIR), str(SCRIPTS_SHARED_DIR)]:
    if p not in sys.path:
        sys.path.insert(0, p)

# ---------------------------------------------------------------------------
# Shared constants
# ---------------------------------------------------------------------------
CI_WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "ci.yml"
TIER5_LAUNCHER_YAML = (
    REPO_ROOT / "tests" / "parity" / "scenarios" / "tier5-launcher.yaml"
)


# ---------------------------------------------------------------------------
# Fixtures: settings.json helpers
# ---------------------------------------------------------------------------


def _write_settings(tmp_path: Path, content: dict) -> Path:
    """Write a settings.json to a temp directory and return its path."""
    p = tmp_path / "settings.json"
    p.write_text(json.dumps(content))
    return p


@pytest.fixture
def settings_path_factory(tmp_path):
    """Factory fixture: create a settings.json with given content in a temp dir."""

    def _factory(content: dict) -> Path:
        return _write_settings(tmp_path, content)

    return _factory


@pytest.fixture
def clean_settings(tmp_path):
    """A settings.json with no amplihack hooks (clean state)."""
    content = {
        "hooks": {
            "PreToolUse": [
                {"type": "command", "command": "/usr/local/bin/other-tool --pre"}
            ]
        }
    }
    return _write_settings(tmp_path, content)


@pytest.fixture
def stale_amplihack_hooks_settings(tmp_path):
    """A settings.json that still references amplihack-hooks (not clean)."""
    content = {
        "hooks": {
            "PreToolUse": [
                {
                    "type": "command",
                    "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
                }
            ],
            "Stop": [
                {"type": "command", "command": "/usr/local/bin/other-tool --stop"}
            ],
        }
    }
    return _write_settings(tmp_path, content)


@pytest.fixture
def empty_array_hooks_settings(tmp_path):
    """A settings.json with a hook key pointing to an empty list (not clean)."""
    content = {"hooks": {"PreToolUse": [], "Stop": []}}
    return _write_settings(tmp_path, content)


@pytest.fixture
def mixed_hooks_settings(tmp_path):
    """Settings with both stale amplihack hooks and non-amplihack hooks."""
    content = {
        "hooks": {
            "PreToolUse": [
                {
                    "type": "command",
                    "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks PreToolUse",
                },
                {"type": "command", "command": "/usr/local/bin/other-hook"},
            ],
            "Stop": [
                {
                    "type": "command",
                    "command": "/home/user/.amplihack/tools/amplihack/amplihack-hooks Stop",
                }
            ],
        }
    }
    return _write_settings(tmp_path, content)
