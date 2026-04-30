"""Path-based entry point for amplihack-hooks ``post-tool-use`` (issue #505).

Forwards the Claude Code hook payload on stdin to the canonical native
implementation ``amplihack-hooks post-tool-use``.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _shim import delegate  # noqa: E402

if __name__ == "__main__":
    sys.exit(delegate("post-tool-use"))
