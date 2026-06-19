# Workflow Provider Abstraction

> [Home](../index.md) > [Features](README.md) > Workflow Provider Abstraction

Workflow provider abstraction lets the same `default-workflow` run on GitHub,
Azure DevOps, and explicit manual-provider paths without embedding provider
logic in recipe shell steps.

## What this feature does

Amplihack separates workflow concepts from provider commands:

| Layer | Responsibility |
| --- | --- |
| `amplihack-workflows` | Pure Rust workflow models, terminal states, provider capabilities, and transition rules. |
| `amplihack-cli` | Provider adapters and `amplihack workflow ...` helper commands. |
| Recipes | Orchestrate steps, call helpers, validate JSON, and keep judgment-heavy work agentic. |

The result is provider-aware behavior without GitHub-only assumptions:

- GitHub issue and pull-request operations use GitHub adapters when configured;
  otherwise helpers return `ManualRequired` instead of fake success.
- Azure Boards operations use Azure DevOps adapters when configured.
- Azure Repos pull-request publication returns `ManualRequired`; this design does
  not automate `az repos pr create`.
- Manual-provider paths return `ManualRequired` instead of calling the wrong
  provider.
- Terminal/finalization state is explicit in logs, recipe JSON, and workflow
  output.

## Quick start

Run the workflow normally:

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout" \
  -c repo_path=. \
  --format json
```

Inspect provider routing directly:

```bash
amplihack workflow detect-provider --repo . --format json
```

Example output:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "DetectProvider",
  "status": "Succeeded",
  "next_action": "No further provider setup is required.",
  "warnings": [],
  "data": {
    "capabilities": {
      "tracking_items": "Automated",
      "change_requests": "Automated",
      "stale_cleanup": "Automated"
    }
  }
}
```

## Helper commands

The workflow helper surface is intentionally small:

```bash
amplihack workflow detect-provider --repo . --format json
amplihack workflow change-request publish --provider github --source-branch feat/timeout --base-branch main --title "Fix timeout" --format json
amplihack workflow change-request publish --provider azure-devops --source-branch feat/timeout --base-branch main --title "Fix timeout" --format json
amplihack workflow terminal-state --terminal-state MANUAL_REQUIRED --format json
amplihack workflow cleanup-stale --provider github --dry-run --format json
amplihack workflow simulate-recipe default-workflow --scenario github-success --format json
```

Recipes consume these commands instead of parsing provider CLI text directly.

## Provider behavior

| Repository | Tracking item | Change request | Cleanup | Terminal output |
| --- | --- | --- | --- | --- |
| GitHub | Automated issue reuse/create when configured | Automated pull request when configured; otherwise `ManualRequired` | Dry-run plan; apply requires a wired provider adapter or manual close action | `Succeeded`, `ManualRequired`, `BlockedManualProvider`, or a terminal failure state |
| Azure DevOps | Azure Boards when configured; local/manual when unavailable | `ManualRequired`; no Azure Repos PR mutation in `default-workflow` | Dry-run/manual; no Azure Repos cleanup mutation in this design | `MANUAL_REQUIRED` or `BLOCKED_MANUAL_PROVIDER` with `next_action` |
| Manual | Manual action | Manual publication action (`status=ManualRequired`) | No remote cleanup | Provider-neutral next action |

## Agentic steps

Adaptive tasks remain agentic when judgment is required. Examples:

- deciding whether an old PR is superseded
- classifying a final workflow state from several evidence sources
- identifying the safest next action after provider metadata is incomplete

Agentic steps must emit JSON contracts. Deterministic validators reject missing
fields, unknown states, low-confidence success, and unsupported provider actions.

## See also

- [Provider-Neutral Workflow Architecture](../concepts/multi-provider-workflow-architecture.md)
- [Provider-Neutral Workflow API](../reference/workflow-provider-contract.md)
- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)
- [Recipe Simulation Reference](../reference/workflow-simulation.md)
