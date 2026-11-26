---
name: amplihack:unlock
disable-model-invocation: true
version: 1.0.0
description: Disable continuous work mode and resume normal behavior
triggers:
  - "Disable continuous work mode"
  - "Stop working autonomously"
  - "Exit lock mode"
  - "Allow Claude to stop"
---

# Unlock: Disable Continuous Work Mode

Disable continuous work mode to allow Claude to stop normally.

When unlocked, Claude will:

- Stop when appropriate based on task completion
- Follow normal stop behavior
- Allow user interaction for next steps

Use this command to exit continuous work mode after `/amplihack:lock` was enabled.

---

## Instructions

Use the Bash tool to run the Rust CLI:

```bash
amplihack unlock
```

This removes `<project>/.claude/runtime/locks/.lock_active` and `.lock_message`
from the same folder, and preserves any handwritten `.continuation_prompt`.
The project folder is `$CLAUDE_PROJECT_DIR`, or the current directory when that
variable is unset.

Run `amplihack lock-status` to check the lock. Enable it again with
`amplihack lock [--message TEXT]`.
