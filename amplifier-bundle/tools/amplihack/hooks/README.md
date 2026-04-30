# amplihack hook scripts (path-based shims)

The Python files in this directory exist to satisfy consumers that
invoke amplihack hooks by absolute path under
`~/.amplihack/.claude/tools/amplihack/hooks/<script>.py`. Each script is
a thin shim that forwards stdin/stdout/stderr to the canonical native
implementation `amplihack-hooks <subcommand>` (see `_shim.py`).

The native binary remains the single source of truth for hook behavior;
these shims exist only so the path-based contract that recipe-runner
templates and Claude Code `settings.json` references can reach the same
code. See issue #505 for the bug that motivated staging these scripts.

When adding a new hook event, copy `post_tool_use.py` and change the
delegate subcommand. When the native binary gains a new subcommand for
a previously-unsupported event (e.g., a real `session-end` subcommand),
update the corresponding shim's `delegate(...)` argument to point at it.
