# Smart-Orchestrator Recovery

How the smart-orchestrator detects failures, recovers via adaptive
strategies, and avoids false positives through hollow-success detection.

## Contents

- [Failure taxonomy](#failure-taxonomy)
- [Recovery pipeline](#recovery-pipeline)
- [Hollow-success detection](#hollow-success-detection)
- [Issue dedup integration](#issue-dedup-integration)
- [Recursion guard](#recursion-guard)
- [Goal status values](#goal-status-values)

---

## Failure taxonomy

The smart-orchestrator (`smart-orchestrator.yaml`) encounters seven failure
modes during execution:

| Mode | Cause | Detection |
|---|---|---|
| **Routing gap** | All execution conditions evaluate to false | `round_1_result` is empty after all execution steps |
| **Agent timeout** | Claude/Copilot session exceeds `timeout_seconds` | Runner kills subprocess, step returns error |
| **Bash failure** | Shell step exits non-zero | `set -euo pipefail` propagates to runner |
| **Recipe failure** | Nested recipe returns exit code 1 | Runner captures sub-recipe exit |
| **Hollow success** | Agent completes but produces no real work | Reflection step detects empty artifacts |
| **Recursion limit** | Session depth exceeds `AMPLIHACK_MAX_DEPTH` | Session tree blocks spawn |
| **Infrastructure** | `gh` unavailable, auth expired, disk full | Various — often detected by bash guard steps |

## Recovery pipeline

Recovery proceeds through four stages, implemented across multiple recipe
steps:

### Stage 1: Detect execution gap

Step `detect-execution-gap` fires when the task type is Development or
Investigation but `round_1_result` is empty:

```yaml
condition: |
  ('Development' in task_type or 'Investigation' in task_type) and not round_1_result
```

This step:
1. Logs a diagnostic banner to stderr
2. Determines the appropriate fallback recipe (`default-workflow` or
   `investigation-workflow`) based on task type
3. Sets `adaptive_recipe` context variable

### Stage 2: File infrastructure bug

Step `file-routing-bug` runs when `adaptive_recipe` is non-empty. It:
1. Assembles diagnostic context (task type, workstream count, environment)
2. Attempts to create a GitHub issue labeled `bug`
3. Falls back to writing a local diagnostic file if `gh` is unavailable

```yaml
condition: |
  adaptive_recipe and adaptive_recipe != ''
```

### Stage 3: Execute adaptive strategy

Two conditional steps attempt direct recipe invocation:

- `adaptive-execute-development` → runs `default-workflow` recipe
- `adaptive-execute-investigation` → runs `investigation-workflow` recipe

These bypass the normal routing logic entirely, providing a last-resort
execution path.

### Stage 4: Reflect with context

The reflection step (Phase 3 in the orchestrator) receives `adaptive_recipe`
and `infra_error_details` as context. It factors the adaptive strategy into
its goal evaluation, noting what failed and what recovery was applied.

## Hollow-success detection

The reflection step distinguishes real results from hollow successes. A
result is hollow when **all three** conditions are true:

1. The agent explicitly states it cannot access the codebase or files
2. No concrete code changes, findings, or artifacts are described
3. The output contains no file paths, diffs, test results, or code
   references

**The reflection step must NOT flag hollow when:**

- Results are partial but real (use `PARTIAL` instead)
- Some criteria are met but not others
- The agent produced specific findings, even if brief
- The task was investigative and concrete conclusions were reached

This distinction prevents false infrastructure failures when agents produce
legitimate but minimal output.

## Issue dedup integration

Currently, the `file-routing-bug` step creates issues with **no dedup
guards**. Each routing failure files a new issue regardless of whether an
identical one exists. This is the primary source of duplicate issue noise.

The [Issue Deduplication](../reference/issue-dedup.md) reference documents
both the existing `default-workflow` guards (which the smart-orchestrator
lacks) and the proposed Rust-side fingerprint dedup that would cover both
recipes.

## Recursion guard

The smart-orchestrator respects session tree limits to prevent infinite
sub-orchestration:

| Variable | Default | Purpose |
|---|---|---|
| `AMPLIHACK_TREE_ID` | (auto) | Shared tree ID for the session |
| `AMPLIHACK_SESSION_DEPTH` | `0` | Current nesting depth |
| `AMPLIHACK_MAX_DEPTH` | `3` | Maximum allowed depth |
| `AMPLIHACK_MAX_SESSIONS` | `10` | Maximum concurrent sessions |

When depth reaches the limit, the orchestrator blocks sub-workstream
spawning and falls back to single-session execution. Session state is
tracked in `/tmp/amplihack-session-trees/{tree_id}.json`.

## Goal status values

The reflection step ends with exactly one status:

| Status | Meaning | Next action |
|---|---|---|
| `ACHIEVED` | All criteria met with evidence | Proceed to summary |
| `PARTIAL -- [what's missing]` | Some criteria met | Trigger round 2 |
| `NOT_ACHIEVED -- [reason]` | No criteria met | Trigger round 2 |
| `HOLLOW -- [what was empty]` | Agent ran but produced nothing | Flag infrastructure issue |

The orchestrator runs up to 3 rounds (configurable) before accepting the
best result.

## Related

- [Issue Deduplication](../reference/issue-dedup.md) — Dedup guards and proposed fingerprint system
- [Recipe Execution Flow](./recipe-execution-flow.md) — Step-by-step execution semantics
- [Recipe Runner Architecture](./recipe-runner-architecture.md) — External binary model
