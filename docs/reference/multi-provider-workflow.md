# Multi-Provider Workflow Reference

> [Home](../index.md) > Reference > Multi-Provider Workflow

Reference for the multi-provider detection and routing logic in
`default-workflow`. This feature enables the same 23-step workflow to operate
against GitHub, Azure DevOps (AzDO), or a local-only repository without
requiring the user to select a provider manually.

> **Note**: This documentation is written retcon-style ahead of full
> implementation. The design is finalized; code changes are in progress.
> See [Issue #684](https://github.com/rysweet/amplihack-rs/issues/684).

## Contents

- [Overview](#overview)
- [Provider Detection (Step 01b)](#provider-detection-step-01b)
- [Issue Tracking by Provider](#issue-tracking-by-provider)
- [Issue Extraction (Step 03b)](#issue-extraction-step-03b)
- [PR Creation Routing (Step 16)](#pr-creation-routing-step-16)
- [Commit Message Formatting (Step 15)](#commit-message-formatting-step-15)
- [Worktree Resume Fix (Step 04)](#worktree-resume-fix-step-04)
- [Context Variables](#context-variables)
- [Diagnostics](#diagnostics)
- [Error Handling](#error-handling)
- [Configuration](#configuration)
- [Examples](#examples)
- [Troubleshooting](#troubleshooting)
- [Related](#related)

---

## Overview

Prior to this change, `default-workflow` assumed GitHub as the sole hosting
provider. Steps 03, 03b, 15, 16, and 21 called `gh` CLI commands
unconditionally, causing failures when the remote pointed to Azure DevOps or
when no remote existed at all.

The multi-provider workflow introduces a **detect-once, branch-everywhere**
pattern: step `01b` runs early in `workflow-prep` to detect the remote
provider, and all downstream steps consult the resulting `remote_provider`
context variable to choose the correct code path.

```
git remote get-url origin
        │
        ▼
┌─────────────────────────────────┐
│  Step 01b: Provider Detection    │
│  *.github.com*  → "github"       │
│  *dev.azure.com*│*vscom* → "azdo"│
│  everything else → "local"       │
└─────────────────────────────────┘
        │
        ▼
  remote_provider = "github" | "azdo" | "local"
        │
        ├─► Step 03:  issue creation (routed)
        ├─► Step 03b: issue extraction (routed)
        ├─► Step 15:  commit format (routed)
        ├─► Step 16:  PR creation (routed)
        └─► Step 21:  PR readiness (routed)
```

---

## Provider Detection (Step 01b)

**Location**: `workflow-prep.yaml`, after step `01` (prepare workspace).

**Logic**:

```bash
REMOTE_URL=$(git remote get-url origin 2>/dev/null || echo "")
case "$REMOTE_URL" in
  *github.com*)         PROVIDER="github" ;;
  *dev.azure.com*|*visualstudio.com*) PROVIDER="azdo" ;;
  *)                    PROVIDER="local"  ;;
esac
```

**Outputs**:

| Variable         | Type   | Description                                    |
| ---------------- | ------ | ---------------------------------------------- |
| `remote_provider` | string | `"github"`, `"azdo"`, or `"local"`             |
| `azdo_org_url`   | string | Full AzDO org URL (empty if not AzDO)           |
| `azdo_org`       | string | AzDO organization name (empty if not AzDO)      |
| `azdo_project`   | string | AzDO project name (empty if not AzDO)            |

**Context propagation**: These variables are outputs of step `01b` in
`workflow-prep.yaml`. For them to propagate to downstream sub-recipes, they
must be declared in the parent `default-workflow.yaml` context block with
empty-string defaults. Without this declaration, the recipe-runner's
`_execute_sub_recipe()` context-merging will not thread them through to
phases like `workflow-publish` or `workflow-finalize`.

Step `01b` must be placed in `workflow-prep.yaml` (steps 00–03b phase) since
that is where provider detection logically belongs — before any step that
needs to know the provider.

**Edge cases**:

- No remote configured → `REMOTE_URL` is empty → `PROVIDER="local"`
- Multiple remotes → only `origin` is checked (convention)
- SSH URLs → pattern matching works on both `git@github.com:` and
  `https://github.com/` forms

---

## Issue Tracking by Provider

### GitHub (`remote_provider = "github"`)

Step 03 uses `gh issue create` and `gh issue view` as before. No change
from the existing behavior documented in
[recipe-step-03-idempotency.md](recipe-step-03-idempotency.md).

### Azure DevOps (`remote_provider = "azdo"`)

Step 03 creates an AzDO work item using the `az boards work-item create`
CLI command:

```bash
az boards work-item create \
  --org "$AZDO_ORG_URL" \
  --project "$AZDO_PROJECT" \
  --type "$WORK_ITEM_TYPE" \
  --title "$ISSUE_TITLE" \
  --description "$ISSUE_BODY"
```

**Work item type**: Defaults to `Task` when no type is specified. The agent
may choose a different type (`Bug`, `User Story`, `Feature`, `Epic`) based
on the `classification` field from step 02's clarified requirements. Users
can also override the type by passing `-c work_item_type=Bug` to the recipe.

The output is an AzDO work item URL:

```
https://dev.azure.com/myorg/myproject/_workitems/edit/12345
```

The idempotency guards (Guard 1 and Guard 2 from
[recipe-step-03-idempotency.md](recipe-step-03-idempotency.md)) are adapted:

- **Guard 1**: Extracts `AB#NNNN` or `#NNNN` references; verifies via
  `az boards work-item show --id <N>`
- **Guard 2**: Searches via `az boards query` using WIQL title match

### Local (`remote_provider = "local"`)

Step 03 generates a synthetic issue number for branch naming and commit
messages, without making any network calls:

```bash
# Combine PID and epoch seconds for collision resistance
SYNTHETIC_ID=$(( ($$ * 100000 + $(date +%s)) % 10000000 ))
```

This scheme avoids same-second collisions by mixing the process PID with
the epoch timestamp. The local ID is used only for branch naming
(`feat/issue-<N>-slug`) and commit message references. No tracking system
is updated.

---

## Issue Extraction (Step 03b)

Step 03b's three-tier extraction logic (documented in
[workflow-issue-extraction.md](workflow-issue-extraction.md)) is extended to
handle all three provider URL formats:

| Provider | URL pattern                                               | Extraction regex                    |
| -------- | --------------------------------------------------------- | ----------------------------------- |
| GitHub   | `https://github.com/owner/repo/issues/N`                  | `issues/[0-9]+`                     |
| AzDO     | `https://dev.azure.com/org/project/_workitems/edit/N`      | `_workitems/edit/[0-9]+`            |
| Local    | Bare numeric string from synthetic ID                       | `^[0-9]+$`                          |

The `issue_number` output contract remains unchanged: a plain integer (or
null). Downstream steps never see the provider URL format — they only
consume the numeric ID.

---

## PR Creation Routing (Step 16)

| Provider | Tool                  | Behavior                                                |
| -------- | --------------------- | ------------------------------------------------------- |
| GitHub   | `gh pr create`        | Unchanged from current behavior                          |
| AzDO     | (manual)              | Logs instructions; automated `az repos pr create` planned |
| Local    | (skipped)             | Logs "no remote — skipping PR creation"                  |

**PR creation asymmetry**: GitHub PR creation is fully automated because `gh`
supports draft PRs, labels, and reviewers in a single command. AzDO PR
creation via `az repos pr create` is planned but not yet automated because
the AzDO CLI requires additional parameters (repository ID, target branch
resolution) that vary by project configuration. The workflow logs clear
instructions for manual PR creation in the meantime.

---

## Commit Message Formatting (Step 15)

Commit messages adapt their issue reference syntax per provider:

| Provider | Format                      | Example                           |
| -------- | --------------------------- | --------------------------------- |
| GitHub   | `Fixes #N` / `Closes #N`    | `feat: add auth (Fixes #684)`     |
| AzDO     | `AB#N`                       | `feat: add auth (AB#12345)`        |
| Local    | `[local-N]`                  | `feat: add auth [local-4821937]`   |

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

## Context Variables

Variables added or modified by the multi-provider feature:

| Variable           | Set by    | Type   | Description                                  |
| ------------------ | --------- | ------ | -------------------------------------------- |
| `remote_provider`  | step-01b  | string | `"github"`, `"azdo"`, or `"local"`            |
| `azdo_org_url`     | step-01b  | string | Full AzDO org URL (empty if not AzDO)         |
| `azdo_org`         | step-01b  | string | AzDO organization name (empty if not AzDO)    |
| `azdo_project`     | step-01b  | string | AzDO project name (empty if not AzDO)          |
| `work_item_type`   | user/step-02 | string | AzDO work item type; default `"Task"`       |
| `issue_number`     | step-03b  | int    | Numeric ID (unchanged contract)               |

**Parent recipe declaration**: All new context variables (`remote_provider`,
`azdo_org_url`, `azdo_org`, `azdo_project`, `work_item_type`) must be
declared with empty-string defaults in `default-workflow.yaml`'s `context:`
block for propagation to work across sub-recipe boundaries.

---

## Diagnostics

All diagnostic output goes to **stderr**.

| Message                                                    | When                                   |
| ---------------------------------------------------------- | -------------------------------------- |
| `INFO: Detected provider: github`                          | step-01b completed detection            |
| `INFO: Detected provider: azdo (org=X, project=Y)`        | step-01b detected AzDO remote           |
| `INFO: Detected provider: local (no remote or unknown)`    | step-01b fallback to local              |
| `INFO: Creating AzDO work item (type=Task)`                | step-03 AzDO path                       |
| `INFO: Skipping PR creation — no remote provider`          | step-16 local path                      |
| `WARN: AzDO work item creation failed — falling back`      | step-03 AzDO error fallback             |

---

## Error Handling

| Failure mode                          | Behavior                                          |
| ------------------------------------- | ------------------------------------------------- |
| `git remote get-url origin` fails     | `remote_provider` = `"local"` (safe fallback)      |
| `az boards` not installed             | Step-03 falls back to local synthetic ID           |
| `az boards` auth expired              | Warning logged; falls back to local synthetic ID   |
| AzDO work item creation fails         | Warning logged; synthetic ID used for branch naming |
| `gh` not installed (GitHub remote)    | Existing behavior: step-03 fails with clear error  |

The principle is: **GitHub steps fail loudly** (because `gh` is a
prerequisite), while **AzDO and local steps degrade gracefully** (because
AzDO support is additive and local is the universal fallback).

---

## Configuration

No new configuration is required for GitHub repositories. The feature
activates automatically based on the remote URL.

For AzDO repositories, ensure:

1. Azure CLI is installed: `az --version`
2. DevOps extension is present: `az extension list | grep devops`
3. Authentication is current: `az login`
4. Defaults are configured: `az devops configure --defaults organization=... project=...`

For local repositories (no remote), no configuration is needed.

Override the detected provider:

```bash
amplihack recipe run default-workflow \
  -c remote_provider=local \
  -c task_description="Work without any remote" \
  -c repo_path=.
```

---

## Examples

### Example 1: GitHub repository (unchanged behavior)

```bash
# Remote: https://github.com/myorg/myrepo.git
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout #684" \
  -c repo_path=.
```

Step 01b detects `github`. Steps 03, 15, 16, 21 use `gh` CLI as before.

### Example 2: Azure DevOps repository

```bash
# Remote: https://dev.azure.com/myorg/myproject/_git/myrepo
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout" \
  -c repo_path=.
```

Step 01b detects `azdo`, extracts org/project. Step 03 creates an AzDO work
item. Step 15 uses `AB#N` format. Step 16 logs manual PR instructions.

### Example 3: Local repository (no remote)

```bash
cd ~/my-local-project && git init
amplihack recipe run default-workflow \
  -c task_description="Add config parser" \
  -c repo_path=.
```

Step 01b detects `local`. Step 03 generates synthetic ID. Steps 15 and 16
use local formatting / skip PR creation.

---

## Troubleshooting

**Provider detected as `local` when a remote exists**

Check that the remote is named `origin`: `git remote -v`. Only the `origin`
remote is inspected. Rename if needed: `git remote rename <old> origin`.

**AzDO work item creation fails with auth error**

Run `az login` and `az devops configure --defaults`. The workflow falls back
to a synthetic ID but logs a warning with remediation steps.

**`az boards` command not found**

Install the DevOps extension: `az extension add --name azure-devops`.

---

## Related

- [Step 03 Idempotency Guards](recipe-step-03-idempotency.md) — GitHub-specific issue creation guards
- [Workflow Issue Extraction (Step 03b)](workflow-issue-extraction.md) — three-tier extraction logic
- [Worktree Reattach and Prune (Step 04)](recipe-step-04-worktree-reattach-prune.md) — worktree lifecycle
- [Azure DevOps Integration](../azure-devops/README.md) — AzDO CLI tools and setup
- [Azure OpenAI Integration](../AZURE_INTEGRATION.md) — Azure model configuration
- [How to Use the Workflow with Azure DevOps](../howto/use-workflow-with-azure-devops.md) — task-oriented AzDO guide
- [Multi-Provider Workflow Architecture](../concepts/multi-provider-workflow-architecture.md) — design decisions

---

**Metadata**

| Field       | Value                                                     |
| ----------- | --------------------------------------------------------- |
| Status      | In Progress (retcon documentation; implementation pending) |
| Issue       | #684                                                      |
| Recipe file | `amplifier-bundle/recipes/workflow-prep.yaml`              |
| Parent      | `amplifier-bundle/recipes/default-workflow.yaml`           |
