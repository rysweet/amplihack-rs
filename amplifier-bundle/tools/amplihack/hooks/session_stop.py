"""Path-based entry point for the ``session-stop`` hook (issue #505).

Several Claude Code template variants emit a ``SessionStop`` event in
addition to (or instead of) ``Stop``. Because amplihack-hooks treats
both as the same lifecycle moment, we delegate to the native ``stop``
subcommand so identical work runs regardless of which event name the
caller wires up. Keeping a dedicated file under the canonical hooks/
directory satisfies the path-based contract the recipe runner enforces.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _shim import delegate  # noqa: E402

if __name__ == "__main__":
    sys.exit(delegate("stop"))
