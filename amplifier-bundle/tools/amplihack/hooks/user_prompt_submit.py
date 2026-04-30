"""Path-based entry point for amplihack-hooks ``user-prompt-submit`` (issue #505).

Forwards the Claude Code hook payload on stdin to the canonical native
implementation ``amplihack-hooks user-prompt-submit``.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _shim import delegate  # noqa: E402

if __name__ == "__main__":
    sys.exit(delegate("user-prompt-submit"))
