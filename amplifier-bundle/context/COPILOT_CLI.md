# GitHub Copilot CLI Integration - Context Summary

**Full Documentation**: See [@docs/COPILOT_CLI.md](../../docs/COPILOT_CLI.md) for complete integration guide.

## Adaptive Hook System

amplihack uses an adaptive hook system that detects which platform is calling (Claude Code vs Copilot CLI) and applies appropriate strategies for context injection.

### Platform Detection

Automatically detects the calling platform by checking:

1. Environment variables (`CLAUDE_CODE`, `GITHUB_COPILOT`)
2. Process name patterns
3. Fallback to Claude Code behavior (safe default)

### Context Injection Strategies

| Platform        | Strategy             | Method                                                         |
| --------------- | -------------------- | -------------------------------------------------------------- |
| **Claude Code** | Direct injection     | `hookSpecificOutput.additionalContext` or stdout               |
| **Copilot CLI** | File-based injection | Write to `.github/agents/AGENTS.md` with `@include` directives |

**Claude Code** (Direct Injection):

```json
// Hook returns JSON with context
{
    "hookSpecificOutput": {
        "additionalContext": "User preferences: talk like a pirate"
    }
}
// AI sees context immediately
```

**Copilot CLI** (File-Based Injection):

```bash
# Hook writes to AGENTS.md
cat > ".github/agents/AGENTS.md" <<'EOF'
# Active Agents and Context

@~/.amplihack/.claude/context/USER_PREFERENCES.md
@~/.amplihack/.claude/context/PHILOSOPHY.md
EOF
# Copilot reads file via @include on next request
```

### Why This Workaround is Needed

**Copilot CLI Limitation**: Hook output is ignored for context injection (except `preToolUse` permission decisions).

**Our Solution Benefits**:

- ✅ Preference injection works on both platforms
- ✅ Context loading works everywhere
- ✅ Zero duplication (single Rust implementation)
- ✅ Automatic platform adaptation

**What Works Where**:
| Feature | Claude Code | Copilot CLI | Implementation |
|---------|-------------|-------------|----------------|
| Logging | ✅ Direct | ✅ Direct | Same hooks |
| Blocking tools | ✅ preToolUse | ✅ preToolUse | Same hooks |
| Context injection | ✅ hookOutput | ✅ AGENTS.md | Adaptive strategy |
| Preferences | ✅ hookOutput | ✅ AGENTS.md | Adaptive strategy |

For complete hook capability comparison, see [@docs/HOOKS_COMPARISON.md](../../docs/HOOKS_COMPARISON.md).

## Integration Components

See [@docs/COPILOT_CLI.md](../../docs/COPILOT_CLI.md) for:

- Complete architecture overview
- Available agents and skills
- MCP server configuration
- Git hooks and session hooks
- Testing and troubleshooting
- Philosophy alignment

## Quick Reference

**Architecture**: Single source of truth in `~/.amplihack/.claude/`, symlinked from `.github/`

**Hook Pattern**: Rust hook modules (crates/amplihack-hooks/)

**Key Files**:

- `.github/copilot-instructions.md` - Base Copilot instructions
- `.github/agents/amplihack/` - Symlink to `~/.amplihack/.claude/agents/amplihack/`
- `.github/agents/skills/` - Symlinks to `~/.amplihack/.claude/skills/`
- `.github/hooks/*` - Wrappers calling the native `amplihack-hooks` binary
- `.github/mcp-servers.json` - MCP server configuration
