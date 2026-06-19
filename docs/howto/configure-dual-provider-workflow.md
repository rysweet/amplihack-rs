# How to Configure Provider-Aware Workflow Tracking

> [Home](../index.md) > How-To > Configure Provider-Aware Workflow Tracking

This guide shows how to configure `default-workflow` tracking for GitHub,
Azure DevOps, and local or unsupported remotes.

---

## Prerequisites

All repositories need:

```bash
git --version
git remote get-url origin
```

For large nested workflow runs, keep the supported Node heap setting:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## 1. Configure GitHub Repositories

Use a normal GitHub remote:

```bash
git remote set-url origin https://github.com/acme/service.git
# or
git remote set-url origin git@github.com:acme/service.git
```

Authenticate the GitHub CLI:

```bash
gh auth login
gh auth status
```

Run the workflow:

```bash
amplihack recipe run default-workflow \
  -c "task_description=Fix the authentication timeout bug" \
  -c "repo_path=$(pwd)"
```

Expected behavior:

```text
REMOTE_HOST_TYPE=github
workflow-prep step 03 uses gh issue view/list/create
GitHub label setup is allowed
```

---

## 2. Configure Azure DevOps Repositories

Use one of the supported AzDO remote forms:

```bash
git remote set-url origin https://dev.azure.com/acme/platform/_git/service
# or
git remote set-url origin https://acme.visualstudio.com/platform/_git/service
# or
git remote set-url origin git@ssh.dev.azure.com:v3/acme/platform/service
```

Azure Boards integration is optional. To allow work item reuse or creation,
install and configure the Azure DevOps CLI extension:

```bash
az extension add --name azure-devops
az login
az devops configure --defaults \
  organization=https://dev.azure.com/acme \
  project=platform
```

Run the workflow:

```bash
amplihack recipe run default-workflow \
  -c "task_description=Fix the authentication timeout bug in AB#12345" \
  -c "repo_path=$(pwd)"
```

Expected behavior:

```text
REMOTE_HOST_TYPE=azdo
workflow-prep step 03 uses Azure Boards or local tracking
gh issue commands are not invoked
gh label commands are not invoked
```

If Azure Boards is unavailable, the workflow emits structured local metadata
instead of attempting a GitHub issue operation:

```text
tracking_system=local
tracking_reference=local-12345
tracking_issue=local-12345
issue_creation=local-tracking
issue_number=
```

The local reference is the durable identifier for that run. Step 03b preserves
it only because the reference is local-prefixed, then leaves `issue_number`
empty instead of converting `12345` into a GitHub issue number.

---

## 3. Configure Local or Unsupported Repositories

No provider configuration is required for missing, local-only, malformed, or
unsupported remotes.

```bash
git remote remove origin

amplihack recipe run default-workflow \
  -c "task_description=Add config parser #482193" \
  -c "repo_path=$(pwd)"
```

Expected behavior:

```text
REMOTE_HOST_TYPE=other
workflow-prep step 03 emits structured local metadata
provider CLIs are not invoked
```

---

## 4. Override Host Type for Follow-Up Work

Normal runs should rely on automatic detection. Use an override only when an
external workflow already knows the provider context.

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c "task_description=Address review feedback for the Azure DevOps PR" \
  -c "repo_path=$(pwd)"
```

Expected behavior:

```text
workflow-prep step 03 emits AB#12345
no GitHub issue or label command runs
```

Use `remote_host_type=other` to force local tracking and block provider API
calls. Local tracking uses `tracking_reference` / `tracking_issue`, not a
numeric `issue_number`. `remote_host_type=azure-devops` remains accepted as a
compatibility alias for external callers, but primary examples should use
`azdo`.

---

## Troubleshooting

### AzDO remote is detected as `other`

Check the exact `origin` URL:

```bash
git remote get-url origin
```

Supported AzDO hosts are:

```text
dev.azure.com
visualstudio.com
ssh.dev.azure.com
```

Unsupported or misspelled hosts intentionally use local tracking.

### Workflow tries to create a GitHub issue in an AzDO repository

That violates the provider isolation contract. Confirm the `origin` URL is one
of the supported AzDO forms, then run the workflow from the repository root or
pass `-c repo_path=/path/to/repo`.

As a temporary workaround for follow-up work, pass:

```bash
-c remote_host_type=azdo -c issue_number=<work-item-id>
```

### Azure Boards creation fails

Azure CLI configuration is needed only when you want Azure Boards reuse or
creation. Check it with:

```bash
az account show
az extension list --query "[?name=='azure-devops'].version" -o tsv
az devops configure --list
```

If Azure Boards remains unavailable, the workflow uses local tracking for the
run and still avoids GitHub issue commands.

---

## See Also

- [Provider-aware workflow prep reference](../reference/dual-provider-workflow.md)
- [How to Use the Default Workflow with Azure DevOps](use-workflow-with-azure-devops.md)
- [Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md)
