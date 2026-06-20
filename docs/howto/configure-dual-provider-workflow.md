# Configure Provider-Aware Workflow Tracking

> [Home](../index.md) > How-To > Configure Provider-Aware Workflow Tracking

Provider-aware workflow tracking is configured through the provider-neutral
workflow contract. Use the current guide:

[Configure Provider-Neutral Workflows](configure-provider-neutral-workflows.md)

> [PLANNED - Implementation Pending]
>
> The `amplihack workflow ...` helper command shown here is the target
> provider-neutral interface.

## Quick check

```bash
export NODE_OPTIONS=--max-old-space-size=32768
amplihack workflow detect-provider --repo . --format json
```

The output shows the repository provider and capability states for tracking
items, change requests, and stale cleanup.

## Common provider setup

| Provider | Setup |
| --- | --- |
| GitHub | Configure a GitHub remote and authenticate `gh`. |
| Azure DevOps | Configure an Azure DevOps remote; configure `az` only when Azure Boards automation is required. |
| Local or unsupported | No provider setup; workflow output names manual next actions. |

For the full task-oriented guide, see
[Configure Provider-Neutral Workflows](configure-provider-neutral-workflows.md).
