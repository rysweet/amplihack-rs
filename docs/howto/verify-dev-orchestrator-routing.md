---
title: Verify Dev-Orchestrator Routing
description: Check that Development workstreams route to default-workflow while hybrid non-Development workstreams keep their routes.
last_updated: 2026-06-11
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Verify Dev-Orchestrator Routing

Use this guide to verify the dev-orchestrator routing contract from the command line.

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
      "recipe": "ops-workflow"
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
    "recipe": "ops-workflow"
  },
  {
    "description": "decide",
    "recipe": "consensus-workflow"
  }
]
```

Only the `Development` workstream is rewritten. Non-Development workstreams keep the route selected by classification and decomposition.

## Verify with focused tests

Run the routing helper regression tests:

```bash
NODE_OPTIONS=--max-old-space-size=32768 cargo test -p amplihack-cli orch
```

Run the workflow reliability contract tests:

```bash
NODE_OPTIONS=--max-old-space-size=32768 cargo test -p amplihack-cli --test issue_672_workflow_reliability_contracts
```

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Development workstream keeps `investigation-workflow` | An old binary or stale installed bundle is being used | Run `cargo build -p amplihack-cli`, then run the intended binary explicitly or refresh the install |
| Every workstream becomes `default-workflow` | Top-level classification is being applied too broadly | Ensure each hybrid workstream carries its own `classification`, `task_type`, or `type` |
| `amplihack orch helper` is missing | The installed CLI is too old | Rebuild or reinstall amplihack |
| `jq` is missing | The examples use `jq` for display only | Inspect the workstreams file with another JSON viewer |

## Related

- [Dev-Orchestrator Routing Contract](../reference/dev-orchestrator-routing.md)
- [`amplihack orch run`](../reference/orch-run-command.md)
- [Getting Started with /dev](../tutorials/dev-orchestrator-tutorial.md)
