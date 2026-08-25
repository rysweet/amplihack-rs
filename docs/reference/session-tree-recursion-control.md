# Session-tree recursion control

How amplihack bounds nested agent spawning, where that state lives, and what to do when
a run is refused. See `docs/spec/README.md` for the formal model and its limits.

Nesting is a supported capability. These controls exist to bound it, not to prevent it.

## Where the state lives

One JSON file per tree, under a **durable** directory:

```
$HOME/.amplihack/amplihack-session-trees/<tree_id>.json
```

Override with `AMPLIHACK_SESSION_TREE_DIR`. The root run pins its resolved value into
that variable for every descendant, so a whole tree agrees on one location instead of
each level re-deriving it.

The location must be identical for every process in a tree. It previously derived from
`TMPDIR`, and each nested run is handed a fresh per-run tempdir, so every level saw an
empty tree: the session cap was evaluated against a file that had just been created, and
the store vanished when the run ended (issue #1326). `AMPLIHACK_HOME` is not a valid
anchor either — it resolves to the current worktree's bundle root and varies the same way.

## The controls

| Variable | Default | Meaning |
|---|---|---|
| `AMPLIHACK_MAX_DEPTH` | `3` | Requested recursion ceiling. May only **lower** the tree's sealed ceiling, never raise it. Clamped to `MAX_DEPTH_CEILING` (32). |
| `AMPLIHACK_MAX_SESSIONS` | `10` | Concurrent active sessions per tree. |
| `AMPLIHACK_SESSION_DEPTH` | unset (root) | Current depth. Set by the runner for each child; not for callers to set. |
| `AMPLIHACK_TREE_ID` | generated | Tree identity. Set by the runner for each child. |
| `AMPLIHACK_SESSION_TREE_DIR` | `$HOME/.amplihack/...` | Where tree state lives. |

The root run **seals** the ceiling into the tree's state. Afterwards the environment may
lower it and cannot raise it. This matters because the environment belongs to the agent
being constrained: during the incident behind issue #1326, agents responded to a depth
refusal by re-running with a larger `AMPLIHACK_MAX_DEPTH` and descending one level
further, a ladder of 5 → 6 → 7 → 8 → 9.

## When a run is refused

```
BLOCKED_TERMINAL orchestration_unavailable: depth 3 of max 3 (issue #964/#1326).
This is a POLICY decision, not an infrastructure fault. Retrying, switching recipe,
or setting AMPLIHACK_MAX_DEPTH will NOT change it -- the ceiling is read from the
session-tree state and the environment may only lower it.
DO: complete this step inline and return your result.
```

Exit status **79**, distinct from `1`, so callers can tell a policy refusal from a fault.
Do not retry it. Complete the step inline.

To legitimately allow deeper nesting, raise the ceiling **on the root run**, before the
tree is sealed:

```bash
AMPLIHACK_MAX_DEPTH=5 amplihack recipe run <recipe> ...
```

### "nested run at depth N has no sealed recursion ceiling"

The environment claims the run is nested, but no tree vouches for a ceiling. This is
refused rather than trusted, because trusting it is the bypass.

A stale `AMPLIHACK_SESSION_DEPTH` left in a shell by a killed run does **not** trigger
this: a depth claim is only acted on when corroborated by a tree id or by a live
orchestrator ancestor. Uncorroborated, it is treated as leftover state and a fresh tree
starts at depth 0. If you hit this and believe it is wrong, check for a stale
`AMPLIHACK_TREE_ID` naming a tree that no longer exists.

## Retention

The store is durable, so it needs an owner:

```bash
amplihack session-tree gc --older-than-days 7 --dry-run   # report
amplihack session-tree gc --older-than-days 7             # remove
```

Removes tree state and lock files whose last activity predates the window. Worth running
periodically on long-lived hosts; the previous `TMPDIR` location got free cleanup, and
that free cleanup is exactly what made the session cap meaningless.

## Mixed-version fleets

`TreeState.writer_version` records the build that sealed a tree. A build predating issue
#1326 resolves the store from `TMPDIR` and will not share a tree with a fixed build, so
both under-count. A mismatch logs a warning rather than refusing, so rolling upgrades
still work — but **the controls are only fully effective once every `amplihack` on `PATH`
is a fixed build.** A partial rollout leaves ceilings unsealed. Check with `which -a
amplihack`.
