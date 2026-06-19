# Clean Up Stale Workflow Change Requests

> [Home](../index.md) > How-To > Clean Up Stale Workflow Change Requests

Use stale cleanup to close or mark superseded workflow-owned change requests
through the provider abstraction. Always run dry-run first.

## Prerequisites

```bash
git status --short --branch
amplihack workflow detect-provider --repo . --format json
```

Provider behavior:

| Provider | Cleanup behavior |
| --- | --- |
| GitHub | Dry-run reports eligible superseded pull requests; apply requires a wired GitHub adapter or manual close action. |
| Azure DevOps | Dry-run reports candidates; apply returns `ManualRequired` with an Azure Repos manual action. |
| Local or unsupported | Reports local/manual next actions and does not mutate remote state. |

## 1. Run dry-run cleanup

```bash
amplihack workflow cleanup-stale \
  --repo . \
  --scope default-workflow \
  --dry-run \
  --format json > cleanup-dry-run.json
```

Inspect candidates:

```bash
jq '.data.candidates[] | {id, decision, action, replacement}' cleanup-dry-run.json
```

Example:

```json
{
  "id": "791",
  "decision": "Superseded",
  "action": "CloseWithComment",
  "replacement": "https://github.com/acme/service/pull/812"
}
```

## 2. Apply cleanup

Apply only after the dry-run output matches the intended cleanup:

```bash
amplihack workflow cleanup-stale \
  --repo . \
  --scope default-workflow \
  --from-dry-run cleanup-dry-run.json \
  --format json
```

The helper mutates provider state only when the scoped identity, supersession
decision, provider capability, and dry-run candidate all match.

## 3. Handle manual providers

Azure DevOps and unsupported providers may return:

```json
{
  "schema_version": 1,
  "operation": "CleanupStale",
  "status": "ManualRequired",
  "provider": "AzureDevOps",
  "next_action": "Close Azure Repos PR 791 as superseded by PR 812 and include the generated cleanup comment.",
  "warnings": [],
  "data": {
    "manual_action": {
      "kind": "CloseSupersededChangeRequest",
      "id": "791",
      "replacement": "812"
    }
  }
}
```

Perform the provider action manually, then rerun dry-run. The candidate list
should be empty or show `NoOp`.

## Safety rules

Cleanup never acts on broad search results alone. A candidate must match scoped
workflow identity:

- repository
- source branch
- base branch
- workflow run or workstream identity
- tracking item
- replacement change request when superseded

If scope is missing or ambiguous, cleanup returns `BlockedManualProvider` or
`Failed` with `next_action`.

## See also

- [Provider-Neutral Workflow API](../reference/workflow-provider-contract.md#stale-cleanup-model)
- [Scoped Workflow Closure](../concepts/scoped-workflow-closure.md)
- [Recipe Simulation Reference](../reference/workflow-simulation.md)
