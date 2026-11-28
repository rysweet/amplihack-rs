---
name: amplihack:lock
disable-model-invocation: true
version: 1.0.0
description: Enable continuous work mode without stopping
triggers:
  - "Enable continuous work mode"
  - "Work autonomously"
  - "Don't stop until done"
  - "Keep working through all tasks"
---

# Lock: Enable Continuous Work Mode

**Purpose**: Enable continuous work mode to prevent Claude from stopping until explicitly unlocked.

**Usage**: `/amplihack:lock [TEXT]`

Pass any text typed after the slash command to `amplihack lock` as `--message`.

When locked, Claude will:

- Use the Bash tool to run `amplihack lock`.

**Basic usage (default continuation prompt):**

```bash
amplihack lock
```

**With custom instruction:**

```bash
amplihack lock --message "Focus on security fixes first"
```

- Continue working through all TODOs and next steps
- Block stop attempts and keep pursuing the user's objective
- Look for additional work and execute in parallel
- Not stop until `/amplihack:unlock` is run

Use this mode when you want Claude to work autonomously through a complex task without stopping.

## Custom Continuation Messages

Pass the optional message with `--message` to provide a custom instruction that Claude sees when attempting to stop.
This enables:

- Task-specific guidance
- Context about what to prioritize
- Domain-specific instructions
- Direction for autonomous work

**Example custom messages:**

```
"Focus on security fixes first, then performance optimizations"
"Check all API endpoints for authentication issues"
"Run full test suite after each change"
"When condition X is met, you may remove the lock file."
```

Messages must be nonempty after trimming and at most 1000 UTF-8 bytes after
trimming. Invalid messages cause an error without changing existing lock state.
The Stop hook uses the trimmed `.lock_message` first, then a usable handwritten
`.continuation_prompt`, then its default prompt. The same text requirements
apply to both files.

---

## Instructions

Use the Bash tool to run the Rust CLI:

**Basic usage (default continuation prompt):**

```bash
amplihack lock
```

**With custom instruction:**

```bash
amplihack lock --message "Focus on security fixes first"
```

**Lock files:**

The project folder is `$CLAUDE_PROJECT_DIR`, or the current directory when that
variable is unset.

- Lock flag: `<project>/.claude/runtime/locks/.lock_active`
- Custom message: `<project>/.claude/runtime/locks/.lock_message`

Run `amplihack lock-status` to check the lock and `amplihack unlock` to disable it.
See [hook state and message precedence](../../../reference/hook-state-root.md#project-configuration) for the project controls.
