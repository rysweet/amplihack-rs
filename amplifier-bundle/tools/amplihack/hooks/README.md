# amplihack hook subcommands (native Rust)

The path-based Python shims that previously lived in this directory have
been deleted (issue #522). Every hook event is now served directly by
subcommands of the native `amplihack-hooks` binary, which is the single
source of truth for hook behavior on every supported host (Claude Code,
Amplifier, Copilot).

## Wiring `settings.json`

Reference the native binary by name in `settings.json` instead of a
filesystem path:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "amplihack-hooks stop", "timeout": 30000 }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          { "type": "command", "command": "amplihack-hooks post-tool-use" }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          { "type": "command", "command": "amplihack-hooks session-end" }
        ]
      }
    ]
  }
}
```

`session-end` and `session-stop` are clap aliases for `stop` — both
dispatch to the same `StopHook` so callers wired to the legacy
`SessionEnd` / `SessionStop` event names keep working without an
additional native handler.

`amplihack install` rewrites any stale `settings.json` that still
references `~/.claude/tools/amplihack/hooks/<name>.py` to the equivalent
native command on next install. Users who hand-edited `settings.json`
should re-run `amplihack install` to trigger the auto-heal.

## Pre-commit (`precommit-prefs`)

`precommit-prefs` is a no-op subcommand invoked by **git** (typically
from `.git/hooks/pre-commit`), not by Claude Code. It drains stdin and
exits 0; it does not log, echo, or persist the payload.

```bash
# .git/hooks/pre-commit
#!/usr/bin/env bash
amplihack-hooks precommit-prefs < /dev/null
```

If a future native subcommand is added to perform real pre-commit work,
the dispatcher's `precommit-prefs` arm becomes a one-line forward.

## API

The Rust crate exposes hook entry points behind module paths under
`amplihack_hooks::*`:

- `amplihack_hooks::stop::StopHook::process` — Stop / SessionEnd / SessionStop
- `amplihack_hooks::post_tool_use::PostToolUseHook::process` — PostToolUse
- `amplihack_hooks::user_prompt::UserPromptSubmitHook::process` — UserPromptSubmit
- `amplihack_hooks::precommit_prefs::run` — pre-commit no-op drain

## Removed files

The following shims were removed in issue #522 (each is replaced by the
named subcommand of `amplihack-hooks`):

| Removed file              | Native replacement                  |
| ------------------------- | ----------------------------------- |
| `_shim.py`                | (internal helper — no longer used)  |
| `stop.py`                 | `amplihack-hooks stop`              |
| `post_tool_use.py`        | `amplihack-hooks post-tool-use`     |
| `user_prompt_submit.py`   | `amplihack-hooks user-prompt-submit`|
| `session_end.py`          | `amplihack-hooks session-end`       |
| `session_stop.py`         | `amplihack-hooks session-stop`      |
| `precommit_prefs.py`      | `amplihack-hooks precommit-prefs`   |

`session_start.py` is out of scope for issue #522 and is still served by
its existing path-based hook elsewhere in the install pipeline.
