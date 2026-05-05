# WIQL Query Guide

Use `az boards query --wiql` to query Azure DevOps work items.

## Common Queries

Assigned to me:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title], [System.State] FROM workitems WHERE [System.AssignedTo] = @Me ORDER BY [System.ChangedDate] DESC" \
  --output table
```

Unassigned active work:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title] FROM workitems WHERE [System.AssignedTo] = '' AND [System.State] <> 'Closed'" \
  --output table
```

High-priority bugs:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title], [Microsoft.VSTS.Common.Priority] FROM workitems WHERE [System.WorkItemType] = 'Bug' AND [Microsoft.VSTS.Common.Priority] = 1" \
  --output table
```

Recently created:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title] FROM workitems WHERE [System.CreatedDate] >= @Today - 7 ORDER BY [System.CreatedDate] DESC" \
  --output json
```

## WIQL Shape

```sql
SELECT [System.Id], [System.Title], [System.State]
FROM workitems
WHERE [System.WorkItemType] = 'Task'
ORDER BY [System.ChangedDate] DESC
```

Use `--output json` when automation needs to parse IDs, titles, or states.
