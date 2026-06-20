# Tutorial: Simulate Provider-Neutral Workflows

> [Home](../index.md) > Tutorials > Simulate Provider-Neutral Workflows

This tutorial shows how to validate a workflow path without live provider calls.

## What you will do

You will run a deterministic simulation for an Azure DevOps repository where
Azure Boards tracking succeeds and Azure Repos pull-request creation requires a
manual action.

## 1. Run the scenario

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack workflow simulate-recipe default-workflow \
  --scenario azdo-work-item-manual-pr \
  --repo-fixture tests/fixtures/workflows/azdo-repo \
  --format json > simulation.json
```

## 2. Inspect the provider result

```bash
jq '.provider, .data.terminal_state, .data.terminal_success, .next_action' simulation.json
```

Expected output:

```text
"AzureDevOps"
"MANUAL_REQUIRED"
false
"Create an Azure Repos pull request from the pushed branch."
```

## 3. Confirm forbidden calls did not run

```bash
jq '.data.forbidden_calls' simulation.json
```

Expected output:

```json
[
  "gh.issue.create",
  "gh.pr.create"
]
```

The simulation fails if the recipe invokes those calls. Listing them here means
the simulator watched for them and confirmed they were not used.

## 4. Validate the finalizer contract

```bash
jq '.data.agent_contracts.finalizer' simulation.json > finalizer.json

amplihack workflow validate-agent-contract \
  --contract finalization \
  --input finalizer.json \
  --format json
```

Expected output includes:

```json
{
  "status": "Succeeded",
  "contract": "finalization"
}
```

## 5. Use the result

The scenario proves the recipe handles this path correctly:

- Azure Boards tracking uses the Azure DevOps adapter.
- GitHub commands are not invoked.
- Azure Repos publication is explicit `MANUAL_REQUIRED`, not fake success.
- Final output includes `next_action`.

For reference details, see [Recipe Simulation Reference](../reference/workflow-simulation.md).
