# Claude Code Hooks vs GitHub Copilot CLI Hooks

**Type**: Explanation (Understanding-Oriented)

Comprehensive comparison of hook systems across the two AI coding platforms
supported by amplihack. Based on official documentation and empirical testing.

## Hook Types Comparison

| Hook Type         | Claude Code          | Copilot CLI            | Notes                           |
| ----------------- | -------------------- | ---------------------- | ------------------------------- |
| **Session Start** | SessionStart         | sessionStart           | Both fire at session begin      |
| **Session End**   | Stop                 | sessionEnd             | Both fire at session end        |
| **Subagent End**  | SubagentStop         | *Not available*        | Claude Code only                |
| **User Prompt**   | UserPromptSubmit     | userPromptSubmitted    | Both fire on prompt submit      |
| **Pre-Tool**      | PreToolUse           | preToolUse             | Both fire before tool execution |
| **Post-Tool**     | PostToolUse          | postToolUse            | Both fire after tool execution  |
| **Permission**    | PermissionRequest    | *Not available*        | Claude Code only                |
| **Error**         | *Not available*      | errorOccurred          | Copilot CLI only                |
| **Notification**  | Notification         | *Not available*        | Claude Code only                |
| **Pre-Compact**   | PreCompact           | *Not available*        | Claude Code only                |
| **TOTAL**         | 10 hooks             | 6 hooks                | Claude Code more comprehensive  |

## Capabilities Comparison

### Context Injection (Adding Information to AI)

| Hook                 | Claude Code                            | Copilot CLI                      |
| -------------------- | -------------------------------------- | -------------------------------- |
| **SessionStart**     | `additionalContext` or stdout          | Output ignored                   |
| **UserPromptSubmit** | `additionalContext` or stdout          | Output ignored                   |
| **PreToolUse**       | `additionalContext`                    | Only permission decision         |
| **PostToolUse**      | `additionalContext`                    | Output ignored                   |
| **Stop**             | `reason` field                         | Output ignored                   |

**Verdict**: Claude Code can inject context at 5+ hook points; Copilot CLI cannot.

### Permission Control (Blocking Operations)

| Capability               | Claude Code                       | Copilot CLI                |
| ------------------------ | --------------------------------- | -------------------------- |
| **Block tool execution** | PreToolUse `deny`                 | preToolUse `deny`          |
| **Block prompt**         | UserPromptSubmit `block`          | Output ignored             |
| **Block agent stop**     | Stop hook `block`                 | Output ignored             |
| **Block permissions**    | PermissionRequest                 | *Not available*            |
| **Modify tool inputs**   | `updatedInput`                    | *Not supported*            |

**Verdict**: Claude Code provides more comprehensive permission control.

### Logging & Monitoring

Both platforms provide comprehensive logging via session, prompt, and tool hooks.
Claude Code additionally supports subagent tracking via `SubagentStop`.
Copilot CLI has a dedicated `errorOccurred` hook.

## Architecture Differences

### Claude Code Hook Architecture

```
Hook executes -> Returns JSON -> Claude Code injects into context -> AI sees it
```

### Copilot CLI Hook Architecture

```
Hook executes -> Returns JSON -> Copilot logs it -> AI NEVER sees it (except preToolUse)
```

**Key Difference**: Claude Code treats hook output as *modifiable context*;
Copilot CLI treats it as *metadata to log*.

## Implementation in amplihack

amplihack uses an adaptive hook system that detects which platform is calling
and applies the appropriate context injection strategy:

| Platform        | Strategy             | Implementation                                                  |
| --------------- | -------------------- | --------------------------------------------------------------- |
| **Claude Code** | Direct injection     | Returns `hookSpecificOutput.additionalContext`                  |
| **Copilot CLI** | File-based injection | Writes to `.github/agents/AGENTS.md` with `@include` directives |

### How It Works

```
# Claude Code path:
Hook returns { hookSpecificOutput: { additionalContext: "..." } }
-> Claude sees the context immediately

# Copilot CLI path:
Hook writes AGENTS.md with @include directives
-> Copilot reads via @include on next request
```

This adaptive approach means:

- Preference injection works on both platforms
- Context loading works everywhere
- Single hook implementation (platform detection is automatic)
- Zero duplication (same hooks, different output strategies)

### amplihack-rs Hook Delivery

In amplihack-rs, the `amplihack install` command writes hook definitions into
`~/.claude/settings.json` (for Claude Code) and `.github/hooks/` (for Copilot
CLI). The hook scripts themselves are shipped as part of the
`amplifier-bundle/` and resolved at runtime via `resolve-bundle-asset`.

See [Hook Specifications](../reference/hook-specifications.md) for the full
hook configuration reference.

## What Works in Both Platforms

| Capability                 | Claude Code | Copilot CLI |
| -------------------------- | ----------- | ----------- |
| **Logging sessions**       | Yes         | Yes         |
| **Logging tool usage**     | Yes         | Yes         |
| **Blocking dangerous ops** | Yes         | Yes         |
| **Error tracking**         | Yes         | Yes         |
| **Audit trails**           | Yes         | Yes         |

## Summary

| Platform           | Rating | Strengths                                   |
| ------------------ | ------ | ------------------------------------------- |
| **Claude Code**    | 5/5    | Full context control, input modification    |
| **Copilot CLI**    | 3/5    | Good logging, permission control via preToolUse |
| **amplihack**      | 4/5    | Works with both, zero duplication           |

## Sources

- [Claude Code Hooks Reference](https://docs.anthropic.com/en/docs/claude-code/hooks)
- Copilot CLI Hooks Documentation
- Empirical testing across both platforms
