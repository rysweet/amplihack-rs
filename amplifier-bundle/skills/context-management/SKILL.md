---
name: context-management
version: 3.1.0
description: |
  Proactive context window management via token monitoring, intelligent extraction, and selective rehydration.
  Features context health checks, priority-based retention, and recovery guidance.
  Use when approaching token limits or needing to preserve essential context.
  Complements /transcripts, /reflect, session checkpoints, and PreCompact hooks.
---

# Context Management Skill

## Purpose

Preserve the information needed to continue work across long sessions, compaction, crashes, and handoffs. This skill uses the active agent runtime plus a bundled native shell helper for status and snapshot management.

## When to Use

- The conversation is getting long or noisy.
- A task spans multiple phases, repos, or agents.
- You need a compact handoff after a crash or compaction.
- You need to rehydrate only requirements, decisions, files touched, validation state, and blockers.

## Workflow

1. **Assess context health**: identify active goal, constraints, changed files, open todos, validation status, and unresolved questions.
2. **Extract essentials**: keep user requirements, decisions, file paths, commands run, failures, fixes, and next actions.
3. **Drop noise**: summarize verbose logs and omit repeated command output unless it contains an unresolved error.
4. **Checkpoint**: write persistent notes through the active session checkpoint/transcript mechanism when available, or use the bundled helper.
5. **Rehydrate**: restore only the minimum context needed for the next action, then verify against repository state before continuing.

## Output Shape

Use this structure for handoffs:

```markdown
## Goal
## User requirements
## Decisions made
## Files changed or inspected
## Validation
## Remaining work
## Blockers
```

## Native Helper

Use the bundled helper for repeatable context status and snapshots:

```bash
amplifier-bundle/skills/context-management/scripts/context-management.sh status
amplifier-bundle/skills/context-management/scripts/context-management.sh snapshot "handoff before compaction"
amplifier-bundle/skills/context-management/scripts/context-management.sh list
amplifier-bundle/skills/context-management/scripts/context-management.sh show latest
```

Snapshots are written to `${AMPLIHACK_CONTEXT_DIR:-$HOME/.amplihack/context-management}`.

## Native Validation

Use runtime commands and repository checks alongside the helper:

```bash
/transcripts recent
/reflect summarize current session context
git --no-pager status --short
```

If those commands are unavailable, produce the same handoff structure manually from the current conversation and repository state.
