# Command Selection Guide

**User Reference**: This guide helps you choose the right slash command for your workflow.

## Quick Command Finder

**Decision Tree**:

```
What do you need to do?
├─ DEVELOPMENT WORKFLOWS
│  ├─ Quick fix (< 5 min)? → /fix
│  ├─ Build new module? → /modular-build
│  ├─ Full feature development? → /ultrathink
│  └─ Autonomous external loop? → /auto
│     └─ Note: /auto runs in subprocess, /ultrathink in current session
│
├─ DECISION MAKING
│  ├─ Need discussion/debate? → debate-workflow skill
│  │  └─ Interactive facilitated discussion converging to consensus
│  └─ Need expert voting? → consensus-voting skill
│     └─ Independent reviews with weighted voting
│
├─ FAULT TOLERANCE
│  ├─ Need reliability/fallbacks? → cascade-workflow skill
│  ├─ Need critical correctness? → n-version-workflow skill
│  └─ Need decision consensus? → debate-workflow skill
│
└─ INVESTIGATION & IMPROVEMENT
   ├─ Understand codebase? → /investigate
   ├─ Session reflection? → /reflect
   └─ Philosophy compliance? → /analyze
```

## Quick Reference Table

| Task Type            | Command                  | When to Use                                    |
| -------------------- | ------------------------ | ---------------------------------------------- |
| Quick fixes          | `/fix [pattern] [scope]` | Error blocking you, need rapid resolution      |
| Module building      | `/modular-build`         | Creating new self-contained module (brick)     |
| Feature development  | `/ultrathink`            | Multi-step feature with workflow orchestration |
| Autonomous work      | `/auto`                  | Long-running task in external subprocess       |
| Code understanding   | `/investigate <path>`    | Deep dive into codebase structure              |
| Session analysis     | `/reflect`               | Review session and create improvement issues   |
| Philosophy check     | `/analyze <path>`        | Validate code against amplihack principles     |
| Multi-model AI       | `amplihack amplifier`    | Use Amplifier with Claude, GPT-4, etc.         |

## Key Distinctions

### `/ultrathink` vs `/auto`

- **`/ultrathink`**: Runs in current Claude Code session (internal orchestration)
- **`/auto`**: Spawns external subprocess with autonomous loop
- **Use `/ultrathink` for**: Interactive development, want to see progress
- **Use `/auto` for**: Hands-off background work, long-running tasks

### `/ultrathink` vs `/modular-build`

- **`/ultrathink`**: General-purpose development workflow
- **`/modular-build`**: Specialized for module creation (progressive pipeline)
- **Use `/ultrathink` for**: Features, bug fixes, general development
- **Use `/modular-build` for**: Specifically creating new bricks (self-contained modules)

## Command Integration Examples

### Quick Workflow: Fix → Build → Review

```bash
/fix import                    # Fix import errors
/modular-build auth-module     # Build new module
/analyze .                     # Check philosophy compliance
```

### Investigation Workflow: Investigate → Ultrathink

```bash
/investigate ./src/core        # Understand existing code
/ultrathink "Add new feature to core"  # Build on understanding
```

## All Available Commands

### Development Commands

- `/fix` - Quick error resolution with pattern detection
- `/modular-build` - Create self-contained modules (bricks)
- `/ultrathink` - Full development workflow
- `/auto` - Autonomous external subprocess

### Investigation Commands

- `/investigate` - Deep codebase exploration
- `/analyze` - Philosophy compliance check
- `/reflect` - Session analysis and improvement

### Document-Driven Development (DDD)

- `/ddd:0-help` - DDD workflow guide
- `/ddd:prime` - Load DDD context
- `/ddd:1-plan` - Planning phase
- `/ddd:2-docs` - Documentation retcon
- `/ddd:3-code-plan` - Code planning
- `/ddd:4-code` - Implementation
- `/ddd:5-finish` - Cleanup and finalize
- `/ddd:status` - Check DDD progress

### Utility Commands

- `/customize` - Manage user preferences
- `/skill-builder` - Create new Claude Code skills
- `/knowledge-builder` - Build knowledge bases
- `/transcripts` - Manage conversation transcripts
- `/lock` / `/unlock` - Continuous work mode
- `/install` / `/uninstall` - System setup

## When in Doubt

Start with `/ultrathink` - it will orchestrate the appropriate workflow for most tasks.

For quick questions or simple tasks, just ask directly without commands.

---

**See Also**:

- `~/.amplihack/.claude/context/AGENT_SELECTION_GUIDE.md` - Which agent to use (AI context)
- `CLAUDE.md` - Complete project documentation
- `~/.amplihack/.claude/commands/` - Individual command documentation
