# Use Default Workflow with Azure DevOps

> [Home](../index.md) > How-To > Use Default Workflow with Azure DevOps

Use this guide to run `default-workflow` in an Azure DevOps repository. The
workflow uses the provider-neutral abstraction, so Azure DevOps behavior is
explicit and GitHub commands are never invoked for Azure DevOps remotes.

> [PLANNED - Implementation Pending]
>
> The `amplihack workflow ...` helper commands shown here are the target
> provider-neutral interface.

## Prerequisites

Set the project heap preference for nested workflow runs:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

Use an Azure DevOps remote:

```bash
git remote set-url origin https://dev.azure.com/acme/platform/_git/service
```

Configure Azure Boards only when you want automated work item reuse or creation:

```bash
az extension add --name azure-devops
az login
az devops configure --defaults \
  organization=https://dev.azure.com/acme \
  project=platform
```

## Run with an existing work item

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout in AB#12345" \
  -c repo_path=. \
  --format json
```

Expected provider result:

```text
provider=AzureDevOps
tracking_item.display_ref=AB#12345
change_request.status=ManualRequired
```

## Run with a new work item

```bash
amplihack workflow tracking-item ensure \
  --repo . \
  --title "Fix authentication timeout" \
  --body-file workflow-body.md \
  --format json
```

When Azure Boards is configured, the result contains an Azure Boards work item.
When Azure Boards is unavailable, the helper returns local/manual or blocked
state with `next_action`.

## Publish the change request

The default Azure DevOps configuration uses Azure Repos PR automation when the
`az` CLI and provider context are available:

```json
{
  "schema_version": 1,
  "provider": "AzureDevOps",
  "operation": "PublishChangeRequest",
  "status": "Succeeded",
  "next_action": "Monitor Azure Repos PR validation.",
  "warnings": [],
  "data": {
    "change_request": {
      "kind": "PullRequest",
      "id": "789",
      "url": "https://dev.azure.com/acme/project/_git/service/pullrequest/789",
      "state": "Open",
      "source_branch": "feat/auth-timeout",
      "base_branch": "main"
    },
    "manual_action": null
  }
}
```

If Azure Repos automation is unavailable, the helper returns a blocked provider
state with an explicit `next_action`; it must not report manual success.
After the PR exists, run terminal-state validation with the PR URL when needed:

```bash
amplihack workflow terminal-state \
  --repo . \
  --branch "$(git branch --show-current)" \
  --base main \
  --change-request-url "https://dev.azure.com/acme/platform/_git/service/pullrequest/456" \
  --format json
```

## Troubleshooting

### Provider is not Azure DevOps

Run:

```bash
amplihack workflow detect-provider --repo . --format json
```

Supported Azure DevOps hosts are `dev.azure.com`, `visualstudio.com`, and
`ssh.dev.azure.com`.

### The workflow reports `BlockedManualProvider`

Read `next_action`. Common causes are missing Azure CLI auth, missing Azure
DevOps extension, missing project defaults, or insufficient permissions. Fix the
provider setup and rerun the helper.

### The workflow reports `ManualRequired`

This is expected when the current provider path is intentionally manual. Perform
the action in `next_action`, then rerun status or terminal-state validation.

## See also

- [Configure Provider-Neutral Workflows](configure-provider-neutral-workflows.md)
- [Provider-Neutral Workflow API](../reference/workflow-provider-contract.md)
- [Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md)
