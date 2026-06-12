# Multi-Provider Workflow Reference

> [Home](../index.md) > Reference > Multi-Provider Workflow

Reference for the multi-provider detection and routing logic in
`default-workflow`. This feature enables the same 23-step workflow to operate
against GitHub, Azure DevOps (AzDO), or a local/unknown repository without
requiring the user to select a provider manually.

## Contents

- [Overview](#overview)
- [Host Detection (Step 02d)](#host-detection-step-02d)
- [Issue Tracking by Provider](#issue-tracking-by-provider)
- [Issue Extraction (Step 03b)](#issue-extraction-step-03b)
- [Commit Message Formatting (Step 15)](#commit-message-formatting-step-15)
- [PR Creation Routing (Step 16)](#pr-creation-routing-step-16)
- [Worktree Resume Fix (Step 04)](#worktree-resume-fix-step-04)
- [Final Status (Step 22b)](#final-status-step-22b)
- [Context Variables](#context-variables)
- [Diagnostics](#diagnostics)
- [Error Handling](#error-handling)
- [Configuration](#configuration)
- [Examples](#examples)
- [Troubleshooting](#troubleshooting)
- [Related](#related)

---

## Overview

`default-workflow` uses a **detect-once, branch-everywhere** pattern for
provider routing. Step `02d` runs in `workflow-prep` to classify the `origin`
remote, and downstream steps consult the resulting `remote_host_type` context
variable before running any provider-specific command.

```
git remote get-url origin
        │
        ▼
┌──────────────────────────────────┐
│  Step 02d: Host Type Detection    │
│  github.com host      → "github"  │
│  Azure DevOps host    → "azdo"    │
│  everything else      → "other"   │
└──────────────────────────────────┘
        │
        ▼
  remote_host_type = "github" | "azdo" | "other"
        │
        ├─► Step 03:  issue creation (routed)
        ├─► Step 03b: issue extraction (routed)
        ├─► Step 15:  commit format (routed)
        ├─► Step 16:  PR creation (routed)
        ├─► Step 21:  PR readiness (routed)
        └─► Step 22b: final status (routed)
```

---

## Host Detection (Step 02d)

**Location**: `workflow-prep.yaml`, step `step-02d-detect-host-type`, after
step `02c` (requirements clarification) and before step `03` (issue creation).

**Target logic**:

```bash
REMOTE_URL=$(git remote get-url origin 2>/dev/null || true)
NORMALIZED_REMOTE_URL=$(printf '%s' "$REMOTE_URL" | tr '[:upper:]' '[:lower:]')

case "$NORMALIZED_REMOTE_URL" in
  https://github.com/*|git@github.com:*|ssh://git@github.com/*)
    REMOTE_HOST_TYPE="github" ;;
  https://dev.azure.com/*|https://*.visualstudio.com/*|git@ssh.dev.azure.com:v3/*|ssh://git@ssh.dev.azure.com/v3/*)
    REMOTE_HOST_TYPE="azdo" ;;
  *)
    REMOTE_HOST_TYPE="other" ;;
esac
```

**Output**: `remote_host_type` — a plain string (`"github"`, `"azdo"`, or
`"other"`) propagated to all downstream sub-recipes via the parent
`default-workflow.yaml` context block. Step 02d only determines the host
type; AzDO-specific URL parsing (organization, project) is performed by
step 03 where the values are consumed.

Step 02d emits `azdo` for Azure DevOps remotes. Downstream steps also accept
`azure-devops` as an equivalent explicit override for contexts supplied by
Azure DevOps PR/work-item follow-up workflows.

The final detector must match complete host shapes. Substring-only matches are
not sufficient because spoofed hosts such as `github.com.evil.example` must be
classified as `other`.

**Edge cases**:

- No remote configured → `REMOTE_URL` is empty → `REMOTE_HOST_TYPE="other"`
- Multiple remotes → only `origin` is checked (convention)
- SSH URLs → explicit patterns match `git@github.com:`, `ssh://git@github.com/`,
  `git@ssh.dev.azure.com:v3/`, and `ssh://git@ssh.dev.azure.com/v3/`
- Not a git repo → step falls back to `"other"` before step 03 runs

---

## Issue Tracking by Provider

### GitHub (`remote_host_type = "github"`)

Step 03 reads `$REMOTE_HOST_TYPE` from context (set by step 02d) and routes
to the GitHub path. Uses `gh issue create` and `gh issue view` as before. No
change from the existing behavior documented in
[recipe-step-03-idempotency.md](recipe-step-03-idempotency.md).

### Azure DevOps (`remote_host_type = "azdo"` or `"azure-devops"`)

When `$REMOTE_HOST_TYPE` is `"azdo"` or `"azure-devops"`, step 03 routes to
the Azure Boards path. It reuses an existing work item before attempting to
create a new one.

**Existing work item reuse**:

| Input | Behavior |
| ----- | -------- |
| `issue_number=N` context value | Emits `AB#N` and exits 0 before GitHub commands, Azure CLI lookup, remote parsing, or work-item creation |
| `task_description` contains `AB#N` | Treats `N` as an Azure Boards candidate; resolves a URL with `az boards work-item show` when available |
| `task_description` contains `#N` | Treats `N` as an Azure Boards candidate because host dispatch already selected Azure DevOps |

The Azure DevOps branch never calls `gh issue view`, `gh issue list`, or
`gh issue create`. This prevents GitHub-specific issue logic from running in
Azure DevOps repositories and keeps ADO PR follow-up work idempotent.

If no existing work item is supplied, step 03 parses the remote URL to extract
the AzDO organization and project names, then creates a work item.

**AzDO URL parsing** (three URL forms):

| URL form | Example | Regex |
| -------- | ------- | ----- |
| HTTPS    | `https://dev.azure.com/org/project/_git/repo` | `dev\.azure\.com/([^/]+)/([^/]+)/` |
| Legacy   | `https://org.visualstudio.com/project/_git/repo` | `([^/.]+)\.visualstudio\.com/([^/]+)/` |
| SSH      | `git@ssh.dev.azure.com:v3/org/project/repo` | `ssh\.dev\.azure\.com[:/]v3/([^/]+)/([^/]+)/` |

**Percent-encoding**: AzDO project names may contain spaces, which appear as
`%20` in URLs. Step 03 decodes `%XX` sequences before validation. The
decoded name is validated against `^[a-zA-Z0-9._ -]+$` (note: spaces are
permitted in decoded form). Invalid percent sequences (e.g., `%ZZ`) cause
the step to fall back to local metadata with a warning.

Step 03 creates the work item using the `az boards work-item create`
CLI command:

```bash
az boards work-item create \
  --org "$AZDO_ORG_URL" \
  --project "$AZDO_PROJECT" \
  --type "$WORK_ITEM_TYPE" \
  --title "$ISSUE_TITLE" \
  --description "$ISSUE_BODY"
```

**Work item type**: Defaults to `Task`. Users can create work items manually
and pass `issue_number=N` or reference them via `AB#N` in the task description.

The output is an AzDO work item URL:

```
https://dev.azure.com/myorg/myproject/_workitems/edit/12345
```

The host-aware idempotency contract is documented in
[step-03-create-issue: Host-Aware Tracking Idempotency](recipe-step-03-idempotency.md).

### Other/Local (`remote_host_type = "other"`)

Step 03 emits structured local metadata without making any network calls:

```text
tracking_system=local
tracking_reference=local-issue-482193
tracking_issue=local-issue-482193
issue_creation=local-tracking
issue_number=482193
```

The local reference is an opaque workflow reference, not a durable provider
record and not a global uniqueness guarantee. It is used only for branch naming
(`feat/issue-<N>-slug`) and commit message references when a numeric ID is
available. No tracking system is updated.

---

## Issue Extraction (Step 03b)

Step 03b's extraction logic (documented in
[workflow-issue-extraction.md](workflow-issue-extraction.md)) handles every
provider output format produced by Step 03:

| Provider | Output pattern | Extraction regex |
| -------- | -------------- | ---------------- |
| GitHub | `https://github.com/owner/repo/issues/N` | `issues/([0-9]+)` |
| GitHub PR fallback | `https://github.com/owner/repo/pull/N` | `pull/([0-9]+)` plus closing-issue lookup |
| Azure DevOps | `https://dev.azure.com/org/project/_workitems/edit/N` | `_workitems/edit/([0-9]+)` |
| Azure DevOps | `AB#N` | `AB#([0-9]+)` |
| Other/local | Structured local metadata with `issue_number=N` or `tracking_reference=local-issue-N` / `local-ab-N` | `issue_number=([0-9]+)` or `local-(issue\|ab)-([0-9]+)` |

The `issue_number` output contract remains unchanged: a plain integer.
Downstream steps never see the provider URL format — they only consume the
numeric ID.

---

## Commit Message Formatting (Step 15)

Commit messages adapt their issue reference syntax based on
`$REMOTE_HOST_TYPE`, consumed from the propagated context variable (set by
step 02d, declared in the parent recipe's context block):

| Host type | Format           | Example                           |
| --------- | ---------------- | --------------------------------- |
| `github`  | `Closes #N`      | `feat: add auth (Closes #684)`    |
| `azdo`    | `AB#N`           | `feat: add auth (AB#12345)`       |
| `azure-devops` | `AB#N`      | `feat: add auth (AB#12345)`       |
| `other`   | `Ref #N`         | `feat: add auth (Ref #4821937)`   |

The `Closes #N` format triggers GitHub's auto-close behavior. The `AB#N`
format triggers Azure Boards work item linking. The `Ref #N` format is a
neutral reference that does not trigger automation on any platform.

---

## PR Creation Routing (Step 16)

| Host type | Tool                  | Behavior                                                |
| --------- | --------------------- | ------------------------------------------------------- |
| `github`  | `gh pr create`        | Unchanged from current behavior                          |
| `azdo`    | (skipped)             | Exits early; user creates PR manually afterward      |
| `azure-devops` | (skipped)        | Same behavior as `azdo`                                  |
| `other`   | (skipped)             | Logs "no remote — skipping PR creation"                  |

Step 16 consumes `$REMOTE_HOST_TYPE` from the propagated context variable
(set by step 02d) rather than re-detecting the host type inline. Non-GitHub
hosts exit early with an informational message. The PR body for GitHub uses
the same host-aware reference format as step 15.

**PR creation asymmetry**: GitHub PR creation is fully automated because `gh`
supports draft PRs, labels, and reviewers in a single command. AzDO PR
creation is skipped — the workflow exits step 16 early with an informational
message on stderr. After the workflow completes, create a PR manually using
`az repos pr create` or the Azure DevOps web UI (see the
[How-To guide](../howto/use-workflow-with-azure-devops.md#create-a-pr-manually)).

---

## Worktree Resume Fix (Step 04)

The worktree setup step (`step-04-setup-worktree`) previously derived the
working directory from the issue URL, which was always a GitHub URL. With
multi-provider support, the worktree path derivation uses only the
`issue_number` integer (provider-agnostic) and the slugified task
description.

See [recipe-step-04-worktree-reattach-prune.md](recipe-step-04-worktree-reattach-prune.md)
for the full worktree lifecycle documentation.

---

## PR Readiness (Step 21)

Step 21 (`step-21-pr-ready`) calls `gh pr ready` and `gh pr comment` —
GitHub-specific commands. Rather than checking `$REMOTE_HOST_TYPE` directly,
step 21 guards on `$PR_URL`: if it is empty or whitespace-only, the step
exits early with an informational message.

This works because non-GitHub hosts never receive a `PR_URL`:

- AzDO/`azure-devops`: step 16 skips automated PR creation → `PR_URL` stays empty
- Other: step 16 skips entirely → `PR_URL` stays empty

As defense-in-depth, step 21 checks `$REMOTE_HOST_TYPE` before `gh` calls to
prevent failures if `PR_URL` is set to a non-GitHub URL through manual context
override.

---

## Final Status (Step 22b)

Step 22b produces a host-aware summary. It checks both `PR_URL` (non-empty)
and `REMOTE_HOST_TYPE` (equals `github`) before invoking any `gh` CLI
commands.

| Host type | PR status line                            | Issue reference line |
| --------- | ----------------------------------------- | -------------------- |
| `github`  | `PR: <url>` (with `gh pr view` details)   | `Issue: #N`          |
| `azdo`    | `PR: N/A (manual creation required)`      | `Issue: AB#N`        |
| `azure-devops` | `PR: N/A (manual creation required)` | `Issue: AB#N` |
| `other`   | `PR: N/A (no remote provider)`            | `Issue: Ref #N`      |

The `HOST_TYPE=${REMOTE_HOST_TYPE:-other}` local variable pattern is used
for `set -u` safety — if the context variable is unset for any reason, the
step degrades to "other" behavior rather than failing.

---

## Context Variables

Variables added or modified by the multi-provider feature:

| Variable           | Set by    | Type   | Description                                  |
| ------------------ | --------- | ------ | -------------------------------------------- |
| `remote_host_type` | step-02d or caller | string | `"github"`, `"azdo"`, `"azure-devops"`, or `"other"` |
| `azdo_org_url`     | step-03   | string | Full AzDO org URL (local to step-03; empty if not AzDO) |
| `azdo_org`         | step-03   | string | AzDO organization name (local to step-03; empty if not AzDO) |
| `azdo_project`     | step-03   | string | AzDO project name, percent-decoded (local to step-03; empty if not AzDO) |
| `work_item_type`   | user/step-02 | string | AzDO work item type; default `"Task"`       |
| `issue_number`     | step-03b  | int    | Numeric ID (unchanged contract)               |

**Parent recipe declaration**: The `remote_host_type` context variable must
be declared with an empty-string default in `default-workflow.yaml`'s
`context:` block. This is required for propagation to work across sub-recipe
boundaries — without this declaration, the recipe-runner's
`_execute_sub_recipe()` context-merging will not thread the value through to
phases like `workflow-publish` or `workflow-finalize`. The AzDO-specific
variables (`azdo_org`, `azdo_project`, `azdo_org_url`) do NOT require parent
declaration because they are local to step-03 within `workflow-prep`.

---

## Diagnostics

All diagnostic output goes to **stderr**. The table below is the expected
diagnostic contract for the Issue #718 host-aware implementation; exact wording
should not be treated as a stable public API.

| Message                                                          | When                                     |
| ---------------------------------------------------------------- | ---------------------------------------- |
| `INFO: Remote host type: github`                                  | step-02d completed detection              |
| `INFO: Remote host type: azdo`                                    | step-02d detected AzDO remote             |
| `INFO: Remote host type: other`                                   | step-02d fallback (no remote/unknown)     |
| `INFO: Reusing work item AB#N`                                    | step-03 reused an existing Azure Boards work item |
| `INFO: Creating AzDO work item (org=X, project=Y, type=Task)`    | step-03 AzDO path                         |
| `INFO: Non-GitHub remote (azdo) — skipping draft PR creation`     | step-16 non-GitHub path                   |
| `INFO: PR_URL is empty ... — skipping gh pr ready`                | step-21 non-GitHub path                   |
| `WARN: AZDO_PROJECT contains unexpected characters — falling back to local tracking` | step-03 validation failed  |
| `WARN: az boards work-item create failed — falling back to local tracking`           | step-03 AzDO error fallback |
| `WARN: Percent-decode failed for project name — falling back to local tracking`      | step-03 invalid `%XX` sequence |

---

## Error Handling

| Failure mode                          | Behavior                                          |
| ------------------------------------- | ------------------------------------------------- |
| `git remote get-url origin` fails     | `remote_host_type` = `"other"` (safe fallback)     |
| `remote_host_type=azure-devops`       | Treated exactly like `azdo`                        |
| Azure DevOps path has `issue_number`  | Reuses `AB#N`; does not call `gh` or create a work item |
| `az boards` not installed             | Step-03 falls back to structured local metadata    |
| `az boards` auth expired              | Warning logged; falls back to structured local metadata |
| AzDO work item creation fails         | Warning logged; structured local metadata is used for branch naming when numeric |
| Percent-decode invalid sequence       | Warning logged; falls back to structured local metadata |
| AzDO project name validation fails    | Warning logged; falls back to structured local metadata |
| `gh` not installed (GitHub remote)    | GitHub creation fails clearly unless the failure is a repository access/resolution failure that triggers local metadata fallback |
| `REMOTE_HOST_TYPE` unset in step 22b  | `HOST_TYPE` defaults to `"other"` via `${..:-other}` |
| `PR_URL` empty in step 21            | Step skips `gh pr ready` with info message          |

The principle is: **GitHub provider errors fail loudly except for repository
access/resolution failures**, which fall back to local metadata so work can
continue when `gh` cannot resolve the repository. **AzDO and other steps use
local metadata fallback** because Azure Boards support is additive and "other"
is the universal provider-safe mode.

---

## Configuration

No new configuration is required for GitHub repositories. The feature
activates automatically based on the remote URL.

For AzDO repositories, Azure CLI is optional. Configure it only when you want
Azure Boards reuse or creation:

1. Azure CLI is installed: `az --version`
2. DevOps extension is present: `az extension list | grep devops`
3. Authentication is current: `az login`
4. Defaults are configured: `az devops configure --defaults organization=... project=...`

Without Azure CLI configuration, AzDO repositories still avoid GitHub issue
commands and fall back to structured local metadata.

For local/unknown repositories (no remote or unrecognized host), no
configuration is needed.

Override the detected host type:

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c task_description="Continue Azure DevOps PR follow-up work" \
  -c repo_path=.
```

Use `remote_host_type=other` to force local metadata without provider API
calls. `remote_host_type=azure-devops` remains a compatibility alias for
external contexts, but `azdo` is the primary value.

---

## Examples

### Example 1: GitHub repository (unchanged behavior)

```bash
# Remote: https://github.com/myorg/myrepo.git
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout #684" \
  -c repo_path=.
```

Step 02d detects `github`. Steps 03, 15, 16, 21 use `gh` CLI as before.
Step 15 uses `Closes #684`. Step 22b shows PR details via `gh pr view`.

### Example 2: Azure DevOps repository

```bash
# Remote: https://dev.azure.com/myorg/myproject/_git/myrepo
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout" \
  -c repo_path=.
```

Step 02d detects `azdo`. Step 03 extracts org/project from the remote URL
and creates an AzDO work item. Step 15 uses `AB#N` format. Step 16 logs
manual PR instructions. Step 22b shows `PR: N/A (manual creation required)`.

### Example 3: Azure DevOps follow-up with existing work item

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c task_description="Address review feedback for existing ADO PR" \
  -c repo_path=/worktrees/ado-pr-follow-up
```

Step 03 treats `azdo` as Azure DevOps, emits `AB#12345`, and exits
without entering GitHub issue reuse/create logic.

### Example 4: AzDO with percent-encoded project name

```bash
# Remote: https://dev.azure.com/myorg/My%20Project/_git/myrepo
amplihack recipe run default-workflow \
  -c task_description="Add config parser" \
  -c repo_path=.
```

Step 03 decodes `My%20Project` → `My Project` and validates successfully.
Workflow proceeds as in Example 2.

### Example 5: Local repository (no remote)

```bash
cd ~/my-local-project && git init
amplihack recipe run default-workflow \
  -c task_description="Add config parser #482193" \
  -c repo_path=.
```

Step 02d detects `other`. Step 03 emits structured local metadata. Step 15 uses
`Ref #N`. Step 16 skips PR creation. Step 22b shows
`PR: N/A (no remote provider)`.

---

## Troubleshooting

**Host detected as `other` when a remote exists**

Check that the remote is named `origin`: `git remote -v`. Only the `origin`
remote is inspected. Rename if needed: `git remote rename <old> origin`.

**AzDO work item creation fails with auth error**

Run `az login` and `az devops configure --defaults`. The workflow falls back
to structured local metadata but logs a warning with remediation steps.

**`az boards` command not found**

Install the DevOps extension: `az extension add --name azure-devops`.

**AzDO project name with spaces rejected**

Check that the remote URL uses standard percent-encoding for spaces (`%20`).
Unusual encodings or non-standard characters may be rejected by the
validation regex.

---

## Related

- [Step 03 Host-Aware Tracking Idempotency](recipe-step-03-idempotency.md) — GitHub issue, Azure Boards work-item, and local tracking reuse/create behavior
- [Workflow Issue Extraction (Step 03b)](workflow-issue-extraction.md) — provider-neutral extraction logic
- [Worktree Reattach and Prune (Step 04)](recipe-step-04-worktree-reattach-prune.md) — worktree lifecycle
- [Azure DevOps Integration](../azure-devops/README.md) — AzDO CLI tools and setup
- [Azure OpenAI Integration](../AZURE_INTEGRATION.md) — Azure model configuration
- [How to Use the Workflow with Azure DevOps](../howto/use-workflow-with-azure-devops.md) — task-oriented AzDO guide
- [Multi-Provider Workflow Architecture](../concepts/multi-provider-workflow-architecture.md) — design decisions

---

**Metadata**

| Field       | Value                                           |
| ----------- | ----------------------------------------------- |
| Contract    | Provider-aware workflow-prep routing            |
| Recipe file | `amplifier-bundle/recipes/workflow-prep.yaml`   |
| Parent      | `amplifier-bundle/recipes/default-workflow.yaml` |
