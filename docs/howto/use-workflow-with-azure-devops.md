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

1. Detects `azdo` as the remote host type (step 02d)
2. Creates an AzDO work item of type `Task` (step 03)
3. Uses `AB#N` commit references (step 15)
4. Skips automated PR creation (step 16)
5. Produces a host-aware summary (step 22b)

### Override the work item type

By default, step 03 creates a `Task`. To use a different work item type,
create the work item manually and pass its ID with `issue_number=N` or
reference it via `AB#N` in the task description.

---

## Run the Workflow with an Existing Work Item

Use either an explicit `issue_number` context value or an `AB#N` reference in
`task_description`. Step 03 keeps both forms inside the Azure DevOps branch and
does not create a GitHub issue.

### Explicit work item context

Use this form for Azure DevOps PR follow-up work where the recipe context
already contains a work item ID:

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azure-devops \
  -c issue_number=12345 \
  -c task_description="Address review feedback for the Azure DevOps PR" \
  -c repo_path=.
```

`remote_host_type=azure-devops` is accepted as an alias for `azdo`. Step 03
emits `AB#12345`, step 03b extracts `12345`, and downstream steps use
Azure Boards references such as `AB#12345`. This explicit context form is
trusted and does not require Azure CLI lookup before reuse.

### Work item reference in task text

Reference the work item number in `task_description`:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix the auth bug described in AB#12345" \
  -c repo_path=.
```

Because host dispatch has already selected Azure DevOps, a bare `#12345`
inside the task description is also treated as an Azure Boards candidate rather
than a GitHub issue. Task-text candidates may still require Azure CLI,
organization, project, and work-item resolution before they are reused.

---

## Percent-Encoded Project Names

AzDO project names containing spaces (e.g., `My Project`) appear
percent-encoded in remote URLs as `My%20Project`. Step 03 decodes `%XX`
sequences before validation, so project names with spaces work correctly.

---

## Differences from GitHub Workflow

| Aspect              | GitHub                         | Azure DevOps                         |
| ------------------- | ------------------------------ | ------------------------------------ |
| Host detection      | step 02d → `github`            | step 02d → `azdo`                    |
| Issue creation      | `gh issue create`              | `az boards work-item create`         |
| Commit reference    | `Closes #N`                    | `AB#N`                               |
| PR creation         | Automated (`gh pr create`)     | Skipped (create manually after)      |
| Auth prerequisite   | `gh auth login`                | `az login` + DevOps extension        |
| Idempotency Guard 1 | `gh issue view`               | Explicit `issue_number` emits `AB#N`; task-text candidates may use `az boards work-item show` |
| Existing `issue_number` | Reuses the supplied issue ID | Emits `AB#N`; no GitHub or Azure CLI lookup required |
| Idempotency Guard 2 | `gh issue list --search`      | Host-isolated; no GitHub title search |
| Summary (step 22b)  | `PR: <url>`                   | `PR: N/A (manual creation required)` |

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

**Host detected as `other` instead of `azdo`**

Verify the remote URL contains `dev.azure.com`, `visualstudio.com`, or
`ssh.dev.azure.com`: `git remote get-url origin`. All three URL forms are
detected:

- HTTPS: `https://dev.azure.com/org/project/_git/repo`
- Legacy: `https://org.visualstudio.com/project/_git/repo`
- SSH: `git@ssh.dev.azure.com:v3/org/project/repo`

If the workflow is launched from an external Azure DevOps context that already
knows the host, pass `-c remote_host_type=azure-devops`. The alias routes to
the same Azure Boards path as `azdo`.

**Workflow tries to create or inspect a GitHub issue**

Ensure `remote_host_type` is `azdo` or `azure-devops`. For guaranteed reuse
without Azure CLI lookup, pass the existing work item as `issue_number=N`.
`AB#N` in task text remains Azure DevOps-scoped, but may need Azure Boards
lookup before reuse. Unknown or misspelled host values use local tracking
rather than GitHub.

**`az boards` fails with authentication error**

Run `az login` to refresh credentials. For PAT-based auth:
`az devops login --organization https://dev.azure.com/YOUR_ORG`.

**Work item type not valid**

List available types: `az boards work-item type list --project YOUR_PROJECT`.
Type names are case-sensitive and vary by process template.

**Project name with spaces not recognized**

Ensure the remote URL uses standard percent-encoding (`%20` for spaces).
Step 03 decodes these automatically. Invalid sequences like `%ZZ` are
rejected and the workflow falls back to local tracking.

---

**Metadata**

| Field  | Value     |
| ------ | --------- |
| Status | Issue #718 target contract |
| Issues | #684, #718 |
