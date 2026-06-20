# Tutorial: Provider-Aware Workflow Tracking

> [Home](../index.md) > Tutorials > Provider-Aware Workflow Tracking

This compatibility tutorial uses the provider-neutral workflow helpers. For the
new simulation tutorial, see
[Tutorial: Simulate Provider-Neutral Workflows](provider-neutral-workflow-simulation.md).

> [PLANNED - Implementation Pending]
>
> The `amplihack workflow ...` helper commands shown here are the target
> provider-neutral interface.

## 1. Detect the provider

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack workflow detect-provider --repo . --format json
```

Expected output includes:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "DetectProvider",
  "status": "Succeeded",
  "next_action": "No further provider setup is required.",
  "warnings": [],
  "data": {}
}
```

## 2. Ensure a tracking item

```bash
amplihack workflow tracking-item ensure \
  --repo . \
  --title "Fix authentication timeout" \
  --body-file workflow-body.md \
  --format json
```

GitHub returns a GitHub issue. Azure DevOps returns an Azure Boards work item
when configured. Local and unsupported repositories return local/manual state
with `next_action`.

## 3. Run the workflow

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout" \
  -c repo_path=. \
  --format json
```

The final result includes provider, tracking item, change-request state, terminal
state, and required next action.

## 4. Validate manual Azure DevOps publication

For Azure DevOps repositories, publication output uses `ManualRequired`:

```json
{
  "schema_version": 1,
  "provider": "AzureDevOps",
  "operation": "PublishChangeRequest",
  "status": "ManualRequired",
  "next_action": "Create an Azure Repos pull request from the pushed branch.",
  "warnings": [],
  "data": {
    "change_request": null,
    "manual_action": {
      "kind": "CreateChangeRequest",
      "source_branch": "feat/auth-timeout",
      "base_branch": "main"
    }
  }
}
```

Create the provider PR manually, then rerun terminal-state validation:

```bash
amplihack workflow terminal-state \
  --repo . \
  --branch "$(git branch --show-current)" \
  --base main \
  --format json
```

## See also

- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)
- [Provider-Neutral Workflow API](../reference/workflow-provider-contract.md)
- [Recipe Simulation Reference](../reference/workflow-simulation.md)
