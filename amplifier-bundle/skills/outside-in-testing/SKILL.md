---
name: outside-in-testing
description: |
  Deprecated compatibility alias for qa-team.
  Use when existing recipes or agents still invoke outside-in-testing by name.
  Redirects future work to the qa-team skill for outside-in validation and parity loops.
version: 1.1.0
---

# outside-in-testing (Alias)

`outside-in-testing` is now a compatibility alias for `qa-team`.

Use `qa-team` for all new work. This alias remains so existing workflows, recipes, and skills that still invoke `outside-in-testing` continue to resolve cleanly while the rename propagates.

See `../qa-team/README.md` and `../qa-team/SKILL.md` for the primary documentation.

**tmux in tests and sandboxes:** pass `-S <private socket>` (or use `env -u TMUX tmux -S <private socket>`) on every tmux command. When `$TMUX` is set, as it is inside any tmux pane, tmux uses that socket and ignores `TMUX_TMPDIR`, so `TMUX_TMPDIR=… tmux kill-server` kills the user's own server. Never run `kill-server` or `kill-session -a` without `-S`/`-L`; a PreToolUse hook denies them.
