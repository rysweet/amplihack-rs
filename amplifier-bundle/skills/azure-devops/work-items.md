# Work Item Management Guide

Create and manage Azure DevOps work items with the official Azure CLI extension.

## Work Item Types

List project-specific work item types:

```bash
az boards work-item type list --output table
```

Show fields for a type:

```bash
az boards work-item type show --name "User Story"
```

## Creating Work Items

Basic creation:

```bash
az boards work-item create \
  --type "User Story" \
  --title "Implement user login"
```

With an HTML description and fields:

```bash
az boards work-item create \
  --type Bug \
  --title "Login button not responding" \
  --description "<p>Button click does nothing.</p>" \
  --assigned-to user@example.com \
  --area "MyProject\\Frontend" \
  --iteration "MyProject\\Sprint 1" \
  --fields "Microsoft.VSTS.Common.Priority=1" "Microsoft.VSTS.Common.Severity=1-Critical"
```

### Always assign new work items (avoid orphans)

Never create a work item without an assignee — unassigned items accumulate as
orphaned, hard-to-triage backlog. Assign to the running identity by default:

```bash
# @me resolves to the caller's `az login` UPN
az boards work-item create --type Issue --title "..." --assigned-to @me
```

### default-workflow (workflow-prep.yaml) behavior

The `default-workflow` recipe's issue-creation step (ADO branch of
`amplifier-bundle/recipes/workflow-prep.yaml`) follows these rules:

- **Assignment (never orphaned):** created items are always assigned to an
  identity. If `AZDO_ASSIGNED_TO` is set, that UPN is used; otherwise the
  caller's UPN is resolved **client-side** via `az account show
  --query user.name` (falling back to `az ad signed-in-user show`). The bare
  `@Me` macro is **not** used for creation — it is a WIQL query macro that
  `az boards work-item create --assigned-to` does not reliably expand, so
  passing it can silently leave items unassigned. If no identity can be
  resolved and no override is set, the step **never creates an unassigned
  item**: it reports the reason on stderr (`WARN`, not silently swallowed)
  and degrades to local tracking so the workflow still proceeds.
- **Reuse before create:** when the task references an existing work item
  (`AB#N` / `#N`, captured as `REF_ISSUE_NUM`), that item is reused via
  `az boards work-item show` instead of minting a new one — so repeated runs
  don't spawn duplicate work items.
- **Configurable type (no hardcoded `Task`, `Issue` is Agile-only):** new items
  are created as `${AZDO_WORK_ITEM_TYPE:-Issue}`. The `Issue` default exists
  only in the **Agile** process template. Projects on **Scrum** (use
  `Product Backlog Item`) or **CMMI** (use `Requirement`) **must** set
  `AZDO_WORK_ITEM_TYPE`, otherwise creation fails loudly (see below).
- **Fail loudly:** a genuine `az boards work-item create` failure (auth,
  permissions, invalid type) is reported to stderr and the step exits non-zero.
  It is **not** silently swallowed into local tracking. Only true
  environment-absence (no `az` CLI, or unresolved org/project) legitimately
  falls back to local tracking with a `WARN`.

> **Note:** the AzDO org and project are **not** environment variables — they
> are derived by parsing `git remote get-url origin` (dev.azure.com,
> visualstudio.com, or ssh remotes). Exporting `AZDO_ORG` / `AZDO_PROJECT` has
> no effect.

| Env var               | Default          | Purpose                                              |
| --------------------- | ---------------- | ---------------------------------------------------- |
| `AZDO_ASSIGNED_TO`    | resolved UPN     | Identity assigned to created work items              |
| `AZDO_WORK_ITEM_TYPE` | `Issue` (Agile)  | Work-item type; **required** on Scrum/CMMI templates |

## Linking Work Items

Link a child to a parent:

```bash
az boards work-item relation add \
  --id 5678 \
  --relation-type Parent \
  --target-id 1234
```

## Updating Work Items

```bash
az boards work-item update --id 12345 --state "Active"
az boards work-item update --id 12345 --assigned-to user@domain.com
az boards work-item update --id 12345 --discussion "Fixed issue"
```

Update arbitrary fields:

```bash
az boards work-item update \
  --id 12345 \
  --fields "System.Tags=ui;critical" "Microsoft.VSTS.Common.Priority=1"
```

## Querying Work Items

Use WIQL for reliable filtering:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title], [System.State] FROM workitems WHERE [System.AssignedTo] = @Me ORDER BY [System.ChangedDate] DESC" \
  --output table
```

JSON for automation:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title] FROM workitems WHERE [System.WorkItemType] = 'Bug'" \
  --output json
```

## Common Workflow

Create an epic and feature children:

```bash
epic_id=$(az boards work-item create --type Epic --title "Authentication System" --query id -o tsv)

for feature in "OAuth Integration" "Session Management" "RBAC"; do
  az boards work-item create --type Feature --title "$feature"
done
```

Then link created child IDs with `az boards work-item relation add`.

## Tips

1. Use HTML for descriptions and comments.
2. Query type fields before setting custom fields.
3. Prefer `--query id -o tsv` when scripts need the created work item ID.
4. Validate parent and child IDs before linking.
