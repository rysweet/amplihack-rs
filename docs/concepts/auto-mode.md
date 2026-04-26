# Auto Mode

**Type**: Explanation (Understanding-Oriented)

Auto mode enables autonomous agentic loops with Claude Code or GitHub Copilot
CLI, allowing AI to work through multi-turn workflows with minimal human
intervention.

## Overview

Auto mode orchestrates an intelligent loop that:

1. Clarifies objectives with measurable evaluation criteria
2. Creates detailed execution plans identifying parallel opportunities
3. Executes plans autonomously through multiple turns
4. Evaluates progress after each turn
5. Continues until objective achieved or max turns reached
6. Provides comprehensive summary of work completed

## Usage

### With Claude Code

```bash
# Basic auto mode
amplihack claude --auto -- -p "implement user authentication"

# With custom max turns
amplihack claude --auto --max-turns 20 -- -p "refactor the API module"
```

### With GitHub Copilot CLI

```bash
# Basic auto mode
amplihack copilot --auto -- -p "add logging to all services"

# With custom max turns
amplihack copilot --auto --max-turns 15 -- -p "implement feature X"
```

## How It Works

### Turn 1: Objective Clarification

Auto mode transforms your prompt into a clear objective with evaluation criteria.

- **Input**: Your prompt
- **Output**: Clear objective statement + measurable evaluation criteria

### Turn 2: Planning

Creates a detailed execution plan, identifying:

- Sequential steps
- Parallel opportunities
- Dependencies between tasks

### Turns 3-N: Execution

Each turn:

1. Executes the next step in the plan
2. Evaluates progress against criteria
3. Adjusts plan if needed
4. Continues or terminates

### Final Turn: Summary

Provides:

- Work completed
- Files changed
- Evaluation results against criteria
- Remaining work (if any)

## Session Limits

Auto mode enforces safety limits to prevent runaway sessions:

| Limit                  | Default | Override Env Var              |
| ---------------------- | ------- | ----------------------------- |
| Max turns              | 10      | `--max-turns` flag            |
| Max API calls          | 100     | `AMPLIHACK_MAX_API_CALLS`     |
| Max session duration   | 30 min  | `AMPLIHACK_MAX_SESSION_DURATION` |

Limit overrides are validated: non-positive or non-integer values fall back
to safe defaults with a warning.

## Platform Differences

| Feature              | Claude Code          | Copilot CLI         |
| -------------------- | -------------------- | ------------------- |
| Context injection    | Full (hook-based)    | File-based (AGENTS.md) |
| Tool restriction     | `--disallowed-tools` | Prompt constraint   |
| Multi-turn support   | Native               | Via subprocess loop |

## amplihack-rs Integration

In amplihack-rs, auto mode is invoked through the unified CLI:

```bash
amplihack claude --auto --max-turns 10 -- -p "task description"
```

The Rust binary handles argument parsing, agent binary resolution, and
subprocess management. See [Automode Safety](../concepts/automode-safety.md)
for critical safety guidelines.

## Related

- [Automode Safety](../concepts/automode-safety.md) — critical safety guide for auto mode
- [Recipe Resilience](../concepts/recipe-resilience.md) — how recipes self-heal on failure
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — how recipes execute
