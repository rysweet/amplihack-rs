# Provider-Aware Workflow Tracking

> [Home](../index.md) > [Features](README.md) > Provider-Aware Workflow Tracking

This page is the compatibility entry point for provider-aware workflow tracking.
The current implementation is the provider-neutral workflow abstraction:

- [Workflow Provider Abstraction](workflow-provider-abstraction.md)
- [Provider-Neutral Workflow Architecture](../concepts/multi-provider-workflow-architecture.md)
- [Provider-Neutral Workflow API](../reference/workflow-provider-contract.md)
- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)

## What changed

Provider-aware tracking no longer means recipe-local shell branches for GitHub
and Azure DevOps. Recipes call typed `amplihack workflow ...` helpers and consume
structured JSON.

| Concern | Current contract |
| --- | --- |
| Host detection | `amplihack workflow detect-provider` returns `GitHub`, `AzureDevOps`, or `Manual`. |
| Tracking items | Provider context exposes provider-neutral tracking capability state. |
| Change requests | `amplihack workflow change-request publish/status` returns `ChangeRequest`, `ManualRequired`, or `BlockedManualProvider`. |
| Terminal state | `amplihack workflow terminal-state` emits explicit final states and next actions. |
| Stale cleanup | `amplihack workflow cleanup-stale` dry-runs and applies through the provider abstraction. |

## Provider behavior

| Provider | Tracking behavior | Publication behavior |
| --- | --- | --- |
| GitHub | GitHub issues through the GitHub adapter when configured. | GitHub pull requests through the GitHub adapter when configured; otherwise `ManualRequired`. |
| Azure DevOps | Azure Boards when configured; local/manual/blocked state otherwise. | `ManualRequired`; Azure Repos PR creation is a manual action in the provider-neutral contract. |
| Manual | Manual or local workflow reference. | `ManualRequired` with provider-neutral next action. |

GitHub commands run only inside the GitHub adapter. Azure DevOps commands run
only inside the Azure DevOps adapter. Manual providers do not call remote
provider CLIs.
