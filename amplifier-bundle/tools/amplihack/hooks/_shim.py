"""Compatibility shim helpers for amplihack hook scripts (issue #505).

Recipe runners, Claude Code ``settings.json`` templates, and external
tooling sometimes invoke amplihack hooks by absolute path under
``~/.amplihack/.claude/tools/amplihack/hooks/<script>.py`` rather than
through the ``amplihack-hooks`` native binary. The Python files in this
directory exist to satisfy that path-based contract; each one delegates
to the native binary so both invocation styles produce identical
behavior and the native implementation remains the single source of
truth.

This module provides the shared delegation helper. It deliberately uses
no third-party imports and no silent fallbacks: when the native binary
is missing we forward the hook payload through unchanged and emit a
diagnostic on stderr so the failure is observable in Claude Code's
hook log without blocking the user's session.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from typing import Optional


def _resolve_binary() -> Optional[str]:
    """Return the path to the ``amplihack-hooks`` binary, or ``None``.

    Honors the ``AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`` env override that
    the install pipeline sets when it stages a vendored binary into
    ``~/.amplihack/bin/``. Falls back to ``$PATH`` lookup.
    """
    override = os.environ.get("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH")
    if override and os.path.isfile(override) and os.access(override, os.X_OK):
        return override
    found = shutil.which("amplihack-hooks")
    return found


def delegate(subcommand: Optional[str]) -> int:
    """Forward stdin/stdout/stderr to ``amplihack-hooks <subcommand>``.

    When ``subcommand`` is ``None`` no native equivalent exists; in that
    case we drain stdin (so the parent process does not block on a full
    pipe) and exit 0 with an empty stdout — the canonical "no-op" hook
    response that does not interfere with the Claude Code session.

    Returns the child exit status, or 0 when the binary is unavailable.
    Hooks must never block the session on infrastructure faults; the
    diagnostic on stderr is the loud signal.
    """
    if subcommand is None:
        try:
            sys.stdin.read()
        except OSError:
            pass
        return 0

    binary = _resolve_binary()
    if binary is None:
        sys.stderr.write(
            "amplihack-hooks binary not found on PATH or via "
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH; "
            f"skipping {subcommand} hook (issue #505 shim)\n"
        )
        try:
            sys.stdin.read()
        except OSError:
            pass
        return 0

    try:
        result = subprocess.run(
            [binary, subcommand],
            stdin=sys.stdin,
            stdout=sys.stdout,
            stderr=sys.stderr,
            check=False,
        )
    except OSError as exc:
        sys.stderr.write(
            f"failed to invoke {binary} {subcommand}: {exc} (issue #505 shim)\n"
        )
        return 0
    return result.returncode
