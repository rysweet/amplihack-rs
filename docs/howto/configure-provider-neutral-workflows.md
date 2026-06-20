# Configure Provider-Neutral Workflows

> [Home](../index.md) > How-To > Configure Provider-Neutral Workflows

Use this guide to configure `default-workflow` behavior for GitHub, Azure
DevOps, or explicit manual-provider paths.

## Prerequisites

All repositories need Git:

```bash
git --version
git remote get-url origin
```

Large nested workflow runs use the supported Node heap preference:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

## Check provider detection

Run the provider detector from the repository root:

```bash
amplihack workflow detect-provider --repo . --format json
```

The `provider` field is one of `GitHub`, `AzureDevOps`, or `Manual`. The
`capabilities` field shows whether tracking items, change
requests, and stale cleanup are automated, manual, blocked, or unsupported.

## Configure GitHub

Use a GitHub remote:

```bash
git remote set-url origin https://github.com/acme/service.git
gh auth login
gh auth status
```

Run the workflow:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout" \
  -c repo_path=. \
  --format json
```

Expected provider behavior:

```text
provider=GitHub
tracking_items=Automated
change_requests=Automated
stale_cleanup=Automated
```

## Configure Azure DevOps

Use a supported Azure DevOps remote:

```bash
git remote set-url origin https://dev.azure.com/acme/platform/_git/service
```

Azure Boards integration is optional. Configure it when you want automated work
item reuse or creation:

```bash
az extension add --name azure-devops
az login
az devops configure --defaults \
  organization=https://dev.azure.com/acme \
  project=platform
```

Run the workflow:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout in AB#12345" \
  -c repo_path=. \
  --format json
```

Expected provider behavior:

```text
provider=AzureDevOps
tracking_items=Automated when Azure Boards is configured
change_requests=Automated when Azure Repos is configured
GitHub commands=not invoked
```

When Azure Repos pull-request automation is unavailable, final output includes a
blocked provider state:

```json
{
  "schema_version": 1,
  "operation": "PublishChangeRequest",
  "status": "BlockedManualProvider",
  "provider": "AzureDevOps",
  "next_action": "Install or authenticate the Azure DevOps CLI and rerun publication.",
  "warnings": ["Azure Repos PR creation failed or was unavailable."],
  "data": {
    "change_request": null,
    "manual_action": null
  }
}
```

## Configure manual-provider paths

No provider setup is required.

```bash
git remote remove origin

amplihack recipe run default-workflow \
  -c task_description="Add config parser" \
  -c repo_path=. \
  --format json
```

Expected provider behavior:

```text
provider=Manual
tracking_items=ManualRequired
change_requests=ManualRequired
remote cleanup=ManualRequired
```

Manual-provider paths use the same manual/blocked output pattern and never call
GitHub or Azure DevOps commands accidentally.

## Override provider context

Normal runs should rely on detection. Use explicit context only when an external
automation layer already knows the provider:

```bash
amplihack recipe run default-workflow \
  -c provider=AzureDevOps \
  -c tracking_item_ref=AB#12345 \
  -c task_description="Address review feedback" \
  -c repo_path=. \
  --format json
```

Overrides are validated against the repository. A GitHub override on an Azure
DevOps remote fails closed instead of routing provider commands to the wrong
service.

## Troubleshooting

### Provider detection is unexpected

Inspect the remote:

```bash
git remote get-url origin
```

If the host is misspelled, fix the remote or continue with explicit manual
provider output. Do not route commands to a different provider just to force
automation.

### Azure DevOps work item creation is blocked

Check Azure CLI setup:

```bash
az account show
az extension list --query "[?name=='azure-devops'].version" -o tsv
az devops configure --list
```

If this remains blocked, the workflow returns `BlockedManualProvider` or local
tracking with a `next_action`; it does not fall back to GitHub issue commands.

### Final output says `ManualRequired`

Complete the provider action named in `next_action`, then rerun the relevant
status or finalization command:

```bash
amplihack workflow terminal-state --terminal-state MANUAL_REQUIRED --format json
```

## See also

- [Workflow Provider Abstraction](../features/workflow-provider-abstraction.md)
- [Provider-Neutral Workflow API](../reference/workflow-provider-contract.md)
- [Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md)
