---
title: Environment Variables
---
# Environment variables (inventory)

Selected env vars consumed by the workspace (non-exhaustive; see code for authority).

| Var | Purpose |
|---|---|
| `AMPLIHACK_HOME` | Root of the amplihack install (asset lookup) |
| `AMPLIHACK_AGENT_BINARY` | Which agent binary to use (copilot/claude) |
| `AMPLIHACK_MAX_DEPTH` | Max recursion depth for nested sessions |
| `AMPLIHACK_SESSION_DEPTH` / `AMPLIHACK_TREE_ID` | Session-tree recursion guard |
| `AMPLIHACK_STEP_TIMEOUT` | Optional per-step propagation (opt-in) |
| `AMPLIHACK_NONINTERACTIVE` | Skip interactive prompts |
| `CLAUDECODE` | Unset to allow nested Claude sessions |

> Orphaned/undocumented env vars are tracked as a code-atlas-bughunt issue.
