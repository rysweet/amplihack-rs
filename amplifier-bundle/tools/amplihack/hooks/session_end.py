"""Path-based entry point for the ``session-end`` lifecycle hook (issue #505).

Claude Code emits a ``SessionEnd`` event when a session terminates so
hooks can flush metrics, persist transcripts, or release locks. The
canonical amplihack-hooks native binary does not yet expose a dedicated
``session-end`` subcommand — historically the project routed end-of-
session work through ``stop``. We delegate to ``stop`` here so the path-
based contract that recipe-runner and ``settings.json`` consumers expect
is satisfied without inventing a new native subcommand.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _shim import delegate  # noqa: E402

if __name__ == "__main__":
    sys.exit(delegate("stop"))
