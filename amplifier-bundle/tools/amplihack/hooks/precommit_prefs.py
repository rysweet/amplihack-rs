"""Path-based entry point for the ``precommit-prefs`` hook (issue #505).

Several settings.json templates reference a ``precommit_prefs.py`` hook
that snapshots user preferences before a commit so post-commit cleanup
can restore them. The amplihack-hooks native binary does not currently
expose a dedicated subcommand for this lifecycle moment, so the shim
drains stdin and exits 0 (the canonical no-op response). Shipping the
file at the expected path prevents Claude Code from logging a
hook-not-found error on every commit; if a future native subcommand is
added, switching this delegate to forward to it is a one-line change.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _shim import delegate  # noqa: E402

if __name__ == "__main__":
    sys.exit(delegate(None))
