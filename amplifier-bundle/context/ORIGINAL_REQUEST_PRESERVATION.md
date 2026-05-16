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

- **context preservation module**: Extracts and structures requirements
- **pre-compact hook** (`crates/amplihack-hooks/`): Exports conversation before compaction
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
crates/amplihack-hooks/src/context_preservation/
crates/amplihack-hooks/src/pre_compact/
.claude/runtime/logs/<session_id>/ORIGINAL_REQUEST.md
```

## Golden Rule

**When in doubt, preserve the user's explicit requirements.**
