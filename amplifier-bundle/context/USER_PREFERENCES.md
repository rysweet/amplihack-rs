# User Preferences

**MANDATORY**: These preferences MUST be followed by all agents. Priority #2 (only explicit user requirements override).

## Autonomy

Work autonomously. Follow workflows without asking permission between steps. Only ask when truly blocked on critical missing information.

## Background work and monitoring (MANDATORY)

**NEVER foreground-sleep.** Long waits MUST run as detached background processes (tmux, `bash mode=async detach=true`, or equivalent). Foreground `sleep N` blocks the user from issuing new instructions and is not acceptable.

- Use detached tmux for any orch / recipe runner / build that may take more than ~30 seconds.
- For polling status, use SHORT non-blocking checks (≤ 30s sleep) only when actively iterating, never long blocking sleeps (> 60s).
- Background processes that must persist beyond the session MUST use `detach: true` (or `tmux new-session -d`).
- When delegating long work, return control to the user immediately after launching; do not block on completion.

## Core Preferences

| Setting             | Value                      |
| ------------------- | -------------------------- |
| Verbosity           | balanced                   |
| Communication Style | (not set)                  |
| Update Frequency    | regular                    |
| Priority Type       | balanced                   |
| Collaboration Style | autonomous and independent |
| Auto Update         | ask                        |
| Neo4j Auto-Shutdown | ask                        |
| Preferred Languages | (not set)                  |
| Coding Standards    | (not set)                  |

## Workflow Configuration

**Selected**: DEFAULT_WORKFLOW (`@~/.amplihack/.claude/workflows/DEFAULT_WORKFLOW.md`)
**Consensus Depth**: balanced

Use CONSENSUS_WORKFLOW for: ambiguous requirements, architectural changes, critical/security code, public APIs.

## Behavioral Rules

- **No sycophancy**: Be direct, challenge wrong ideas, point out flaws. Never use "Great idea!", "Excellent point!", etc. See `@~/.amplihack/.claude/context/TRUST.md`.
- **Quality over speed**: Always prefer complete, high-quality work over fast delivery.

## Learned Patterns

<!-- User feedback and learned behaviors are added here by /amplihack:customize learn -->

## Managing Preferences

Use `/amplihack:customize` to view or modify (`set`, `show`, `reset`, `learn`).
