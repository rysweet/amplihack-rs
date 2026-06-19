# Provider-Aware Workflow Prep Reference

> [Home](../index.md) > Reference > Provider-Aware Workflow Prep

This compatibility reference points the old dual-provider workflow prep contract
at the current provider-neutral workflow API.

> [PLANNED - Implementation Pending]
>
> The helper commands shown here are the target provider-neutral interface.

## Current contract

`workflow-prep` uses typed helper commands:

```bash
amplihack workflow detect-provider --repo . --format json
amplihack workflow tracking-item ensure --repo . --title "Fix timeout" --body-file body.md --format json
```

The old `remote_host_type` / `REMOTE_HOST_TYPE` shell context is a compatibility
input only. New recipes consume provider JSON:

```json
{
  "schema_version": 1,
  "provider": "AzureDevOps",
  "operation": "EnsureTrackingItem",
  "status": "Succeeded",
  "next_action": "Use AB#12345 in commits and change-request descriptions.",
  "warnings": [],
  "data": {
    "tracking_item": {
      "kind": "WorkItem",
      "id": "12345",
      "display_ref": "AB#12345"
    }
  }
}
```

## Provider isolation

Provider commands run only inside provider adapters:

| Provider | Allowed adapter commands |
| --- | --- |
| GitHub | GitHub issue and pull-request operations through `gh`. |
| Azure DevOps | Azure Boards operations through `az` when configured. |
| Local | No remote provider commands. |
| Unsupported | No remote provider commands. |

Azure DevOps, local, and unsupported repositories do not fall back to GitHub
commands. Missing automation returns `ManualRequired` or
`BlockedManualProvider` with `next_action`.

## Related documentation

- [Provider-Neutral Workflow API](workflow-provider-contract.md)
- [Multi-Provider Workflow Reference](multi-provider-workflow.md)
- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)
