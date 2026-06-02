# How to Use the Default Workflow with Azure DevOps

> [Home](../index.md) > How-To > Use Workflow with Azure DevOps

Task-oriented guide for running `default-workflow` against an Azure DevOps
repository. For design rationale, see
[Multi-Provider Workflow Architecture](../concepts/multi-provider-workflow-architecture.md).
For full reference, see
[Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md).

---

## Prerequisites

1. **Azure CLI** installed with the DevOps extension:

   ```bash
   az --version
   az extension add --name azure-devops
   ```

2. **Authenticated** to your AzDO organization:

   ```bash
   az login
   az devops configure --defaults \
     organization=https://dev.azure.com/YOUR_ORG \
     project=YOUR_PROJECT
   ```

3. **Git remote** named `origin` pointing to your AzDO repo:

   ```bash
   git remote -v
   # origin  https://dev.azure.com/myorg/myproject/_git/myrepo (fetch)
   ```

---

## Run the Workflow with a New Work Item

```bash
amplihack recipe run default-workflow \
  -c task_description="Add retry logic to API client" \
  -c repo_path=.
```

The workflow automatically:

1. Detects `azdo` as the remote provider (step 01b)
2. Creates an AzDO work item of type `Task` (step 03)
3. Uses `AB#N` commit references (step 15)
4. Logs manual PR creation instructions (step 16)

### Override the work item type

By default, step 03 creates a `Task`. Override for other types:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix null reference in auth module" \
  -c work_item_type=Bug \
  -c repo_path=.
```

Valid types depend on your AzDO process template (Agile, Scrum, Basic, CMMI).
Common types: `Task`, `Bug`, `User Story`, `Feature`, `Epic`.

---

## Run the Workflow with an Existing Work Item

Reference the work item number in `task_description`:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix the auth bug described in AB#12345" \
  -c repo_path=.
```

Or pass the issue number directly to skip step 03:

```bash
amplihack recipe run default-workflow \
  -c issue_number=12345 \
  -c task_description="Fix the auth bug" \
  -c repo_path=.
```

---

## Differences from GitHub Workflow

| Aspect              | GitHub                         | Azure DevOps                         |
| ------------------- | ------------------------------ | ------------------------------------ |
| Issue creation      | `gh issue create`              | `az boards work-item create`         |
| Issue reference     | `#N`                           | `AB#N`                               |
| PR creation         | Automated (`gh pr create`)     | Manual (instructions logged)         |
| Auth prerequisite   | `gh auth login`                | `az login` + DevOps extension        |
| Idempotency Guard 1 | `gh issue view`               | `az boards work-item show`           |
| Idempotency Guard 2 | `gh issue list --search`      | `az boards query` (WIQL)             |

---

## Create a PR Manually

After the workflow completes steps 1–15 (commit and push), create a PR
using the Azure DevOps CLI:

```bash
az repos pr create \
  --source-branch "$(git branch --show-current)" \
  --target-branch main \
  --title "feat: add retry logic (AB#12345)" \
  --description "Implements retry logic per work item AB#12345"
```

Or use the Azure DevOps web UI to create the PR from the pushed branch.

---

## Troubleshooting

**Provider detected as `local` instead of `azdo`**

Verify the remote URL contains `dev.azure.com` or `visualstudio.com`:
`git remote get-url origin`. SSH-style AzDO URLs
(`git@ssh.dev.azure.com:v3/org/project/repo`) are also detected.

**`az boards` fails with authentication error**

Run `az login` to refresh credentials. For PAT-based auth:
`az devops login --organization https://dev.azure.com/YOUR_ORG`.

**Work item type not valid**

List available types: `az boards work-item type list --project YOUR_PROJECT`.
Type names are case-sensitive and vary by process template.

---

**Metadata**

| Field  | Value                                                     |
| ------ | --------------------------------------------------------- |
| Status | In Progress (retcon documentation; implementation pending) |
| Issue  | #684                                                      |
