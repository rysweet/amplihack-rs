---
name: azure-devops
description: |
  Complete Azure DevOps (ADO) automation — work items, boards, sprints, repos, pull requests,
  pipelines, builds, artifacts. Use when user mentions ADO, work items, user stories, bugs,
  sprints, builds, releases, or Azure DevOps URLs.
version: 2.0.0
type: skill
auto_activate_keywords:
  - azure devops
  - work item
  - boards
  - wiql
  - ado
  - ado boards
  - devops work item
  - azure repos
  - pull request
  - azure pipelines
  - azure artifacts
tools_required:
  - az
  - az extension add --name azure-devops
references:
  - name: "Azure DevOps CLI Documentation"
    url: "https://learn.microsoft.com/en-us/cli/azure/devops"
  - name: "az boards work-item Commands"
    url: "https://learn.microsoft.com/en-us/cli/azure/boards/work-item"
  - name: "Work Items REST API"
    url: "https://learn.microsoft.com/en-us/rest/api/azure/devops/wit/work-items"
  - name: "WIQL Syntax Reference"
    url: "https://learn.microsoft.com/en-us/azure/devops/boards/queries/wiql-syntax"
supporting_docs:
  - authentication.md
  - work-items.md
  - queries.md
  - html-formatting.md
  - repos.md
  - pipelines.md
  - artifacts.md
  - HOW_TO_CREATE_YOUR_OWN.md
---

# Azure DevOps Skill

Complete Azure DevOps integration covering boards, repositories, pipelines, and artifacts through the Azure CLI and the official `azure-devops` extension.

**Auto-activates when:** User mentions Azure DevOps, ADO, work items, boards, repos, pipelines, artifacts, or Azure DevOps URLs.

## Quick Start

### 1. Authenticate and configure defaults

```bash
az extension add --name azure-devops
az login
az devops configure --defaults organization=https://dev.azure.com/ORG project=PROJECT
az devops configure --list
```

See [@authentication.md] for setup details.

### 2. Common operations

Create a work item:

```bash
az boards work-item create \
  --type "User Story" \
  --title "Implement feature" \
  --description "<p>Story description</p>"
```

Query work items:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title] FROM workitems WHERE [System.AssignedTo] = @Me ORDER BY [System.ChangedDate] DESC"
```

Create a pull request:

```bash
az repos pr create \
  --source-branch feature/branch \
  --target-branch main \
  --title "Add feature"
```

## Progressive Loading References

- [@authentication.md] - Azure CLI login and project defaults
- [@work-items.md] - Work item creation, updates, links, and queries
- [@queries.md] - WIQL query patterns
- [@html-formatting.md] - Formatting HTML descriptions for Azure DevOps
- [@repos.md] - Repository and pull request operations
- [@pipelines.md] - Pipeline and build operations
- [@artifacts.md] - Package feed operations

## Important Notes

- Azure DevOps work item descriptions are HTML. Convert Markdown to HTML before passing long descriptions.
- Prefer `--output json` when automation needs IDs or URLs.
- Always configure `organization` and `project` defaults before running board/repo commands.
- Use `az devops invoke` only when the official `az boards`, `az repos`, `az pipelines`, or `az artifacts` command group does not expose the needed operation.
