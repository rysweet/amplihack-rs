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
