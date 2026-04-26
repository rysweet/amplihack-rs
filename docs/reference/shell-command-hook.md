# Shell Command Hook

**Type**: Reference (Information-Oriented)

A minimal shell command execution hook for Claude Code. Detects prompts starting
with `!` and executes safe, read-only commands.

## Usage

```
!ls -la     # List files
!pwd        # Current directory
!date       # Current date
!whoami     # Username
```

## Allowed Commands

The hook uses a strict whitelist of 9 read-only commands:

| Command  | Purpose             |
| -------- | ------------------- |
| `cat`    | Display file content |
| `date`   | Current date/time   |
| `echo`   | Print text          |
| `head`   | First lines of file |
| `ls`     | List directory      |
| `pwd`    | Working directory   |
| `tail`   | Last lines of file  |
| `wc`     | Word/line count     |
| `whoami`  | Current user        |

## Behavior

- Prompts starting with `!` are intercepted by the `UserPromptSubmit` hook
- The command is parsed with `shlex.split()` for safe argument handling
- Execution runs in `/tmp` with a 5-second timeout
- The hook blocks prompt submission and returns output in the `reason` field
- Commands not in the whitelist are rejected

## Security

| Protection           | Detail                                  |
| -------------------- | --------------------------------------- |
| Whitelist-only       | Only 9 safe, read-only commands allowed |
| No shell injection   | `shell=False` with argument parsing     |
| Timeout              | 5-second limit per command              |
| Restricted directory | Runs in system temp directory           |
| Cross-platform       | Works on Unix, macOS, and Windows       |

## Implementation

The hook is implemented in `user_prompt_submit.py` (72 lines). It follows
amplihack's ruthless simplicity philosophy:

- Single function, no classes
- Whitelist + timeout, nothing more
- No dead code, no stubs, no placeholders

## Files

| File | Purpose |
| ---- | ------- |
| `amplifier-bundle/tools/amplihack/hooks/user_prompt_submit.py` | Hook implementation |
| `~/.claude/settings.json` | Hook registration |

## Related

- [Configure Hooks](../howto/configure-hooks.md) — how to set up and merge hooks
- [Hooks Comparison](../concepts/hooks-comparison.md) — how hooks work across platforms
- [Hook Specifications](../reference/hook-specifications.md) — full hook configuration schema
