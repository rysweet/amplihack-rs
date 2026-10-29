---
title: Verify Dev-Orchestrator Routing
description: Check that Development workstreams route to default-workflow or quick-change while hybrid non-Development workstreams keep their routes.
last_updated: 2026-10-29
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Verify Dev-Orchestrator Routing

Use this guide to verify the deterministic Development routing contract from the command line: Development workstreams run `default-workflow`, or `quick-change` for small single-workstream tasks. Binaries that predate this behavior may fail these checks by preserving an incorrect model-provided `recipe`.

## Prerequisites

- Run from the repository root.
- Install amplihack so `amplihack orch helper` is available.
- Use an installed bundle that contains `smart-orchestrator`, `smart-classify-route`, and `smart-execute-routing`.

## Verify a Development workstream with a missing recipe

Create a decomposition payload:

```bash
cat > /tmp/dev-routing-missing-recipe.json <<'JSON'
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "missing-recipe",
      "classification": "Development",
      "description": "Fix the routing bug"
    }
  ]
}
JSON
```

Normalize it:

```bash
WS_FILE=$(amplihack orch helper build-workstreams-config < /tmp/dev-routing-missing-recipe.json)
jq '.[0].recipe' "$WS_FILE"
```

Expected output:

```json
"default-workflow"
```

## Verify a Development workstream with the wrong recipe

Create a decomposition payload:

```bash
cat > /tmp/dev-routing-wrong-recipe.json <<'JSON'
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "wrong-recipe",
      "classification": "Development",
      "description": "Add routing regression tests",
      "recipe": "investigation-workflow"
    }
  ]
}
JSON
```

Normalize it:

```bash
WS_FILE=$(amplihack orch helper build-workstreams-config < /tmp/dev-routing-wrong-recipe.json)
jq '.[0].recipe' "$WS_FILE"
```

Expected output:

```json
"default-workflow"
```

## Verify hybrid route preservation

Create a mixed decomposition payload:

```bash
cat > /tmp/dev-routing-hybrid.json <<'JSON'
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "investigate",
      "classification": "Investigation",
      "description": "Trace current routing behavior",
      "recipe": "investigation-workflow"
    },
    {
      "name": "implement",
      "classification": "Development",
      "description": "Enforce Development routing",
      "recipe": "investigation-workflow"
    },
    {
      "name": "answer",
      "classification": "Q&A",
      "description": "Explain the routing contract",
      "recipe": "qa-workflow"
    },
    {
      "name": "operate",
      "classification": "Operations",
      "description": "Run routing diagnostics",
      "recipe": "sentinel-preserved-operations-route"
    },
    {
      "name": "decide",
      "classification": "Consensus",
      "description": "Select the rollout strategy",
      "recipe": "consensus-workflow"
    }
  ]
}
JSON
```

Normalize it:

```bash
WS_FILE=$(amplihack orch helper build-workstreams-config < /tmp/dev-routing-hybrid.json)
jq '[.[] | {description, recipe}]' "$WS_FILE"
```

Expected output:

```json
[
  {
    "description": "investigate",
    "recipe": "investigation-workflow"
  },
  {
    "description": "implement",
    "recipe": "default-workflow"
  },
  {
    "description": "answer",
    "recipe": "qa-workflow"
  },
  {
    "description": "operate",
    "recipe": "sentinel-preserved-operations-route"
  },
  {
    "description": "decide",
    "recipe": "consensus-workflow"
  }
]
```

Only the `Development` workstream is rewritten. Non-Development workstreams keep the route selected by classification and decomposition. The Operations recipe above is a sentinel preservation value for helper-output verification, not a real bundle recipe to execute with `amplihack orch run`.

## Verify top-level fallback for unclassified workstreams

Create a decomposition payload where the workstream has no own `classification`, `task_type`, or `type`:

```bash
cat > /tmp/dev-routing-top-level-fallback.json <<'JSON'
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "fallback",
      "description": "Fix a workstream that relies on top-level classification",
      "recipe": "investigation-workflow"
    }
  ]
}
JSON
```

Normalize it:

```bash
WS_FILE=$(amplihack orch helper build-workstreams-config < /tmp/dev-routing-top-level-fallback.json)
jq '.[0].recipe' "$WS_FILE"
```

Expected output:

```json
"default-workflow"
```

Per-workstream classification wins when present. Top-level `task_type` is used only when the workstream has no own classification, so an unclassified workstream under top-level `Development` routes to `default-workflow`.

## Verify that a quick-change recipe is kept

```bash
cat > /tmp/dev-routing-quick-change.json <<'JSON'
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "fix-link",
      "classification": "Development",
      "description": "Fix the broken link in docs/index.md",
      "recipe": "quick-change"
    }
  ]
}
JSON
WS_FILE=$(amplihack orch helper build-workstreams-config < /tmp/dev-routing-quick-change.json)
jq '.[0].recipe' "$WS_FILE"
```

Expected output:

```json
"quick-change"
```

Force the full workflow with the override that `-c workflow=` sets:

```bash
WS_FILE=$(RECIPE_VAR_workflow=default-workflow \
  amplihack orch helper build-workstreams-config < /tmp/dev-routing-quick-change.json)
jq '.[0].recipe' "$WS_FILE"
```

Expected output:

```json
"default-workflow"
```

## Verify the workflow selection and log line

```bash
printf '%s' '{"recommended_workflow":"quick-change","workflow_reason":"comment typo"}' \
  | amplihack orch helper select-workflow --task-type Development --workstream-count 2
```

Expected stdout, because `quick-change` needs exactly one workstream:

```text
default-workflow
```

Expected stderr:

```text
[dev-orchestrator] Workflow: default-workflow source=classifier reason=quick-change needs one workstream; got 2
```

An override on a non-Development task is ignored:

```bash
printf '%s' '{}' \
  | amplihack orch helper select-workflow --override quick-change --task-type 'Q&A' --workstream-count 1
```

Expected stdout is `none`, and stderr starts with:

```text
[dev-orchestrator] WARNING: workflow override 'quick-change' ignored for task_type Q&A
```

The full precedence table is in [Quick-Change Recipe: Routing from smart-orchestrator](../reference/quick-change-recipe.md#routing-from-smart-orchestrator).

## Verify with focused tests

Run the routing helper regression tests:

```bash
NODE_OPTIONS=--max-old-space-size=32768 cargo test -p amplihack-cli orch
```

Run the quick-change selection and recipe tests:

```bash
NODE_OPTIONS=--max-old-space-size=32768 cargo test -p amplihack-cli --test cli_integration issue_1740
```

Run the workflow reliability contract tests:

```bash
NODE_OPTIONS=--max-old-space-size=32768 cargo test -p amplihack-cli --test cli_integration issue_672_workflow_reliability_contracts::
```

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Development workstream keeps `investigation-workflow` | An old binary or stale installed bundle is being used | Run `cargo build -p amplihack-cli`, then run the intended binary explicitly or refresh the install |
| Every workstream becomes `default-workflow` | Top-level classification is being applied too broadly | Ensure each hybrid workstream that should keep a non-Development route carries its own `classification`, `task_type`, or `type` |
| Development workstream with `recipe: "quick-change"` becomes `default-workflow` | `RECIPE_VAR_workflow=default-workflow` is set in the shell, or the binary predates #1740 | Unset `RECIPE_VAR_workflow`, or rebuild the CLI |
| `WARNING: unknown workflow override` | `-c workflow=` holds a value other than `quick-change` or `default-workflow` | Correct the value; it is matched after trimming and lower-casing |
| `amplihack orch helper` is missing | The installed CLI is too old | Rebuild or reinstall amplihack |
| `jq` is missing | The examples use `jq` for display only | Inspect the workstreams file with another JSON viewer |

## Related

- [Dev-Orchestrator Routing Contract](../reference/dev-orchestrator-routing.md)
- [Quick-Change Recipe](../reference/quick-change-recipe.md)
- [`amplihack orch run`](../reference/orch-run-command.md)
- [Getting Started with /dev](../tutorials/dev-orchestrator-tutorial.md)
