# Original Request Preservation

Ensures original user requirements are preserved and passed to all agents to prevent requirement loss during context compaction.

## Problem

Original user requests get lost during context compaction, leading to agents optimizing away explicit requirements.

## Solution

amplihack approach:

1. **Context Preservation**: Extract and preserve original requirements
2. **Agent Injection**: Include requirements in ALL agent prompts
3. **Conversation Export**: Export before compaction
4. **Validation**: Check preservation at key steps

## Core Components

- **context_preservation.py**: Extracts and structures requirements
- **pre_compact.py**: Exports conversation before compaction
- **Agent injection**: Include requirements in ALL agent prompts

## Agent Context Format

```markdown
## 🎯 ORIGINAL USER REQUEST - PRESERVE THESE REQUIREMENTS

**Target**: [User's stated goal]

**Requirements**: • [List user requirements]
**Constraints**: • [List user constraints]

**CRITICAL**: Do NOT optimize away explicit requirements.
```

## Key Rules

1. **Always include** original request context when invoking agents
2. **Never optimize away** explicit user requirements
3. **Validate preservation** at cleanup steps (6 & 14)
4. **Export conversations** before compaction

## File Locations

```
.claude/tools/amplihack/context_preservation.py
.claude/tools/amplihack/hooks/pre_compact.py
<hook state root>/runtime/sessions/<session_key>/logs/ORIGINAL_REQUEST.md
```

The hook state root is `$AMPLIHACK_HOME` when that is an absolute path with no
`..` component and not a Git checkout (no `.git` directory, file or symlink at
its root), and `$HOME/.amplihack` otherwise. When `AMPLIHACK_HOME` names a
checkout, as it can under `recipe run -w <checkout>`, the file is under
`$HOME/.amplihack/runtime/`, not in the checkout. See the
[Hook State Root Reference](../../reference/hook-state-root.md).

## Golden Rule

**When in doubt, preserve the user's explicit requirements.**
