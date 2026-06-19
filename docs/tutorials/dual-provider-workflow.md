# Tutorial: Run the Default Workflow on an Azure DevOps Repository

**Time to complete**: 15 minutes

This tutorial walks through a `default-workflow` run on an Azure DevOps-backed
repository and shows how `workflow-prep` avoids GitHub issue commands.

## What You'll Learn

1. Verify that an AzDO remote is classified as `azdo`
2. Run `default-workflow` with an existing Azure Boards work item
3. Confirm that tracking uses Azure Boards or structured local metadata
4. Create the Azure DevOps pull request manually after the workflow pushes a branch

---

## Step 1: Prepare the Environment

Use an Azure DevOps repository clone:

```bash
cd /path/to/ado-repo
git remote get-url origin
```

Expected remote examples:

```text
https://dev.azure.com/acme/platform/_git/service
https://acme.visualstudio.com/platform/_git/service
git@ssh.dev.azure.com:v3/acme/platform/service
```

Keep the supported Node heap setting for large workflow runs:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## Step 2: Configure Azure Boards, If You Want Provider Tracking

If you want the workflow to reuse or create Azure Boards work items, configure
the Azure DevOps CLI:

```bash
az extension add --name azure-devops
az login
az devops configure --defaults \
  organization=https://dev.azure.com/acme \
  project=platform
```

If you skip this step, `workflow-prep` still classifies the repo as `azdo`, but
step 03 falls back to local tracking instead of trying GitHub.

---

## Step 3: Run the Workflow with an Existing Work Item

Run `default-workflow` with an Azure Boards reference in the task description:

```bash
amplihack recipe run default-workflow \
  -c "task_description=Fix the session timeout bug described in AB#12345" \
  -c "repo_path=$(pwd)"
```

During `workflow-prep`, the route is:

```text
step-02d-detect-host-type -> REMOTE_HOST_TYPE=azdo
step-03-create-issue -> Azure Boards/local tracking path
```

GitHub issue and label commands are skipped before command construction. The
AzDO path does not run:

```text
gh issue view
gh issue list
gh issue create
gh label list
gh label create
```

---

## Step 4: Read the Tracking Output

When Azure Boards resolves the work item, step 03 emits a parseable Azure
Boards reference:

```text
AB#12345
```

or a full work-item URL:

```text
https://dev.azure.com/acme/platform/_workitems/edit/12345
```

If Azure Boards is unavailable, step 03 emits structured local metadata:

```text
tracking_system=local
tracking_reference=local-12345
tracking_issue=local-12345
issue_creation=local-tracking
issue_number=
```

Provider URLs and `AB#N` preserve the downstream numeric `issue_number`
contract. Local metadata uses a local-prefixed `tracking_reference` /
`tracking_issue`, preserves that reference downstream, and leaves
`issue_number` empty.

---

## Step 5: Let the Workflow Continue

After tracking setup, the normal development steps run against the local git
checkout:

| Phase | Provider behavior |
| ----- | ----------------- |
| Worktree setup | Uses local git only |
| Design and implementation | Provider-independent |
| Tests and pre-commit | Provider-independent |
| Commit and push | Uses normal `git push origin <branch>` |
| PR creation | Automated only for GitHub; AzDO reports manual PR instructions |

Commit references use Azure Boards syntax when the host type is `azdo`:

```text
AB#12345
```

---

## Step 6: Create the Azure DevOps PR Manually

After the workflow pushes the branch, create the PR with Azure DevOps:

```bash
az repos pr create \
  --source-branch "$(git branch --show-current)" \
  --target-branch main \
  --title "Fix the session timeout bug (AB#12345)" \
  --description "Implements the fix for AB#12345."
```

You can also create the PR from the Azure DevOps web UI.

---

## Step 7: Try the Local Fallback Path

To see the provider-safe fallback, run the workflow without Azure Boards
configuration or with `remote_host_type=other`:

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=other \
  -c "task_description=Add config parser" \
  -c "repo_path=$(pwd)"
```

Expected tracking output:

```text
tracking_system=local
tracking_reference=local-482193
tracking_issue=local-482193
issue_creation=local-tracking
issue_number=
```

No GitHub or Azure DevOps provider command runs in this mode.

---

## Summary

You ran the workflow on an Azure DevOps-backed repository. `workflow-prep`
classified the remote as `azdo`, kept all GitHub issue and label commands out
of the AzDO path, preserved numeric `issue_number` for Azure Boards, and
preserved local references without numeric coercion for local tracking.
