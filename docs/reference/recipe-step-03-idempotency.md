# Tracking Item Idempotency Reference

> [Home](../index.md) > Reference > Tracking Item Idempotency

Step 03 compatibility behavior is implemented through
`amplihack workflow tracking-item ensure`. The helper owns deterministic parsing,
provider routing, reuse, creation, local references, and manual/blocked states.

> [PLANNED - Implementation Pending]
>
> The helper command shown here is the target provider-neutral interface.

## Command

```bash
amplihack workflow tracking-item ensure \
  --repo . \
  --title "Fix authentication timeout" \
  --body-file workflow-body.md \
  --format json
```

## Idempotency order

The helper attempts reuse before creation:

1. Validate explicit provider context when supplied.
2. Detect the repository provider from Git remote metadata.
3. Reuse an explicit tracking reference from context or task text when it belongs
   to the detected provider.
4. Search provider records only when the provider adapter supports safe search.
5. Create a provider tracking item only when the adapter capability is
   `Automated`.
6. Return local tracking, `ManualRequired`, or `BlockedManualProvider` when
   automation is unavailable.

## Provider behavior

| Provider | Reuse | Create | Unavailable path |
| --- | --- | --- | --- |
| GitHub | Existing issue URL, `#N`, or an open issue with an identical title. | `gh issue create` through the GitHub adapter, only with `create_issue=true`. | `BlockedManualProvider` when GitHub tooling/auth is required but unavailable. |
| Azure DevOps | `AB#N`, Azure Boards URL, or explicit work item ID. | `az boards work-item create` through the Azure DevOps adapter when configured. | Local/manual/blocked state with `next_action`; never GitHub fallback. |
| Local | Existing `local-*` reference. | New local workflow reference. | No remote calls. |
| Unsupported | Provider-neutral manual reference when allowed. | None. | `ManualRequired` or `BlockedManualProvider`. |

## Current GitHub behaviour in step 03

The `workflow-prep` recipe already follows steps 3–6 above for GitHub:

- An explicit `issue_number` or a validated `#N` is reused first.
- Without `create_issue=true`, step 03 returns local tracking and makes no
  GitHub write.
- With it, step 03 reuses an open issue whose title is exactly the task
  title, and otherwise creates one with bounded retries on rate limits
  (at least 60 s plus jitter between creates on a secondary limit), falling
  back to local tracking when the time budget runs out or when it cannot
  check for an existing issue.

The [Tracking Issue Selection Reference](workflow-tracking-issue-selection.md)
has the details, including the `AMPLIHACK_ISSUE_BACKOFF_*` variables.

## Output

Successful provider tracking output:

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
      "display_ref": "AB#12345",
      "url": "https://dev.azure.com/acme/platform/_workitems/edit/12345"
    }
  }
}
```

Manual output:

```json
{
  "schema_version": 1,
  "provider": "Unsupported",
  "operation": "EnsureTrackingItem",
  "status": "ManualRequired",
  "next_action": "Create or reference a tracking item in the repository's provider and rerun with tracking_item_ref.",
  "warnings": [],
  "data": {
    "tracking_item": null
  }
}
```

## Regression contract

Tests verify:

1. GitHub, Azure DevOps, local, unsupported, and spoofed remotes route through
   the correct provider state.
2. Existing provider references are reused before creation.
3. Local references are never coerced into GitHub issue numbers or Azure Boards
   work item IDs.
4. Azure DevOps and unsupported paths never call GitHub commands.
5. Manual and blocked states include actionable `next_action` text.

## Related documentation

- [Tracking Issue Selection Reference](workflow-tracking-issue-selection.md) - how the current step-03 recipe chooses a GitHub tracking issue
- [Provider-Neutral Workflow API](workflow-provider-contract.md)
- [Multi-Provider Workflow Reference](multi-provider-workflow.md)
- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)
