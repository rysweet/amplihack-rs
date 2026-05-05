# Repository Operations Guide

Work with Azure DevOps repositories and pull requests using `az repos`.

## Listing Repositories

```bash
az repos list --output table
az repos list --output json
```

Show repository details:

```bash
az repos show --repository REPO_NAME --output json
```

## Creating Pull Requests

Basic pull request:

```bash
az repos pr create \
  --source-branch feature/auth \
  --target-branch main \
  --title "Add authentication"
```

With a description file:

```bash
az repos pr create \
  --source-branch feature/auth \
  --target-branch main \
  --title "Add authentication" \
  --description "$(cat pr_description.md)"
```

With reviewers and linked work items:

```bash
az repos pr create \
  --source-branch feature/bug-fix \
  --target-branch main \
  --title "Fix critical bug" \
  --reviewers user1@domain.com user2@domain.com \
  --work-items 12345 12346
```

Draft pull request:

```bash
az repos pr create \
  --source-branch feature/wip \
  --target-branch main \
  --title "WIP: New feature" \
  --draft true
```

## Common Workflow

```bash
git checkout -b feature/new-feature main
# make changes, commit
git push -u origin feature/new-feature

az repos pr create \
  --source-branch feature/new-feature \
  --target-branch main \
  --title "Add new feature" \
  --description "$(cat feature_desc.md)"
```

## Clone URLs

HTTPS:

```text
https://dev.azure.com/ORG/PROJECT/_git/REPO
```

SSH:

```text
git@ssh.dev.azure.com:v3/ORG/PROJECT/REPO
```
