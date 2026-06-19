# Provider-Aware Workflow Prep Reference

> [Home](../index.md) > Reference > Provider-Aware Workflow Prep

Technical contract for host detection and tracking-item dispatch in
`workflow-prep.yaml`.

## Contents

- [Overview](#overview)
- [Host Detection API](#host-detection-api)
- [Step 03 Tracking Dispatch](#step-03-tracking-dispatch)
- [Command Isolation Contract](#command-isolation-contract)
- [Configuration](#configuration)
- [Outputs](#outputs)
- [Local Metadata Contract](#local-metadata-contract)
- [Examples](#examples)
- [Regression Contract](#regression-contract)

---

## Overview

`workflow-prep` detects the repository host once, before tracking setup. The
detected host type determines whether step 03 may call GitHub, Azure Boards, or
only local tracking.

The public contract is the `remote_host_type` recipe context value and its shell
counterpart `REMOTE_HOST_TYPE`.

| Value | Meaning | Provider commands allowed |
| ----- | ------- | ------------------------- |
| `github` | The `origin` remote is hosted by GitHub | `gh issue`, `gh label`, later GitHub PR commands |
| `azdo` | The `origin` remote is hosted by Azure DevOps | `az boards` in the Azure Boards branch only |
| `other` | Missing, unsupported, malformed, or non-GitHub/non-AzDO remote | None |

The classifier emits only `github`, `azdo`, or `other`. Downstream code may
accept `azure-devops` as a compatibility alias from external recipe context,
but step `02d` itself emits `azdo`.

---

## Host Detection API

### Step

`workflow-prep.yaml::step-02d-detect-host-type`

### Input

| Input | Source | Description |
| ----- | ------ | ----------- |
| `repo_path` | Recipe context | Repository path where `origin` is inspected |
| `origin` URL | `git remote get-url origin` | Remote URL to classify |

### Output

| Output | Values | Description |
| ------ | ------ | ----------- |
| `remote_host_type` | `github`, `azdo`, `other` | Host classifier result propagated by `default-workflow` |

### Classification Rules

The finished classifier lowercases and normalizes the remote URL before
matching known host shapes.

| Remote shape | Host type |
| ------------ | --------- |
| `https://github.com/OWNER/REPO.git` | `github` |
| `git@github.com:OWNER/REPO.git` | `github` |
| `ssh://git@github.com/OWNER/REPO.git` | `github` |
| `https://dev.azure.com/ORG/PROJECT/_git/REPO` | `azdo` |
| `https://ORG.visualstudio.com/PROJECT/_git/REPO` | `azdo` |
| `git@ssh.dev.azure.com:v3/ORG/PROJECT/REPO` | `azdo` |
| `ssh://git@ssh.dev.azure.com/v3/ORG/PROJECT/REPO` | `azdo` |
| Empty, missing, unsupported, malformed, or local-only remote | `other` |

The feature contract is host-aware matching. A URL such as
`https://github.com.evil.example/OWNER/REPO.git` is classified as `other`, not
`github`.

---

## Step 03 Tracking Dispatch

### Step

`workflow-prep.yaml::step-03-create-issue`

### Inputs

| Context key | Required | Description |
| ----------- | -------- | ----------- |
| `repo_path` | Yes | Repository path used for local git and provider commands |
| `task_description` | Yes | Task text used for title creation and existing reference extraction |
| `final_requirements` | No | Requirements text included in new provider records when a provider record is created |
| `remote_host_type` | No | Host routing value from step 02d; empty or unknown values are treated as `other` |
| `issue_number` | No | Existing tracking ID; reused before provider create paths |

### Dispatch Matrix

| `REMOTE_HOST_TYPE` | Existing ID behavior | Create behavior | Fallback behavior |
| ------------------ | -------------------- | --------------- | ----------------- |
| `github` | Reuse `#N` after `gh issue view` validates it | `gh issue create`; label setup stays in this branch | Most GitHub creation errors fail; repository access/resolution failures fall back to local metadata |
| `azdo` | Reuse `issue_number=N`, `AB#N`, or host-scoped `#N` as an Azure Boards candidate | `az boards work-item create` when Azure Boards is available | Structured local metadata |
| `azure-devops` | Compatibility alias for `azdo` when supplied by callers | Same as `azdo` | Same as `azdo` |
| `other`, empty, unknown | Preserve local tracking metadata when supplied | No provider creation | Structured local metadata |

The Azure DevOps branch does not call GitHub first and then recover from the
failure. It bypasses GitHub issue and label commands before those commands are
constructed.

---

## Command Isolation Contract

Provider-specific commands are physically scoped to their provider branch.

| Command family | Allowed host type | Forbidden host types |
| -------------- | ----------------- | -------------------- |
| `gh issue view` | `github` | `azdo`, `azure-devops`, `other`, empty, unknown |
| `gh issue list` | `github` | `azdo`, `azure-devops`, `other`, empty, unknown |
| `gh issue create` | `github` | `azdo`, `azure-devops`, `other`, empty, unknown |
| `gh label list/create` | `github` | `azdo`, `azure-devops`, `other`, empty, unknown |
| `az boards work-item show/create` | `azdo`, `azure-devops` | `github`, `other`, empty, unknown |

This prevents task descriptions, branch names, local paths, and provider
metadata from being sent to the wrong service.

---

## Configuration

### GitHub

```bash
gh auth status
git remote get-url origin
# https://github.com/OWNER/REPO.git
```

No recipe flags are required.

### Azure DevOps

Azure Boards integration is optional. If it is configured, the AzDO branch can
reuse or create work items:

```bash
az extension add --name azure-devops
az login
az devops configure --defaults \
  organization=https://dev.azure.com/ORG \
  project=PROJECT
```

If Azure Boards is unavailable, the AzDO branch emits local metadata and
continues without invoking GitHub issue logic.

### Local or unsupported remotes

No provider configuration is required.

### Runtime preference

For large nested workflow runs:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## Outputs

Step 03 writes provider output to stdout. For GitHub and Azure Boards success
paths, that output is a single URL or `AB#N` reference. For local fallback, the
output is multiline key/value metadata.

| Host path | Step 03 output | Step 03b numeric ID |
| --------- | -------------- | ------------------- |
| GitHub | `https://github.com/OWNER/REPO/issues/123` | `123` |
| Azure DevOps existing work item | `AB#12345` | `12345` |
| Azure DevOps work item URL | `https://dev.azure.com/ORG/PROJECT/_workitems/edit/12345` | `12345` |
| Local metadata | `tracking_system=local` plus `tracking_reference=local-482193`, `tracking_issue=local-482193`, `issue_creation=local-tracking`, and `issue_number=` | Empty; local reference is preserved instead |

Downstream workflow steps consume numeric `issue_number` for GitHub and Azure
DevOps. For local tracking, they consume `tracking_reference` /
`tracking_issue`; `issue_number` is empty.

---

## Local Metadata Contract

Local fallback output is structured metadata:

```text
tracking_system=local
tracking_reference=local-482193
tracking_issue=local-482193
issue_creation=local-tracking
issue_number=
```

`tracking_reference` is the durable local identifier for the run. It may use
`local-N`, `local-issue-N`, `local-ab-N`, or another `local-*` value.
`tracking_system=local` is a mode marker, not a complete local reference by
itself. Step 03b checks for a local-prefixed `tracking_reference` /
`tracking_issue` before numeric extraction; when one is present, it preserves
the local reference, emits an empty `issue_number`, and exits successfully.
Numeric-looking local IDs are not GitHub issues and are not Azure Boards work
item IDs.

`local-tracking:N` is accepted as a legacy local reference format and is
preserved as local tracking. Its numeric suffix is not copied to
`issue_number`.

---

## Examples

### GitHub issue reuse

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout #684" \
  -c repo_path=.
```

With a GitHub `origin`, step 03 may run:

```text
gh issue view 684
gh issue list --state open --search ...
gh issue create ...
```

### Azure DevOps work item reuse

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout in AB#12345" \
  -c repo_path=.
```

With an AzDO `origin`, step 03 emits an Azure Boards reference or local
tracking reference. It does not run any `gh issue` or `gh label` command.

### Explicit AzDO context from a follow-up workflow

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c task_description="Address review feedback for the Azure DevOps PR" \
  -c repo_path=.
```

Step 03 emits:

```text
AB#12345
```

### Unsupported remote

```bash
git remote set-url origin https://gitlab.com/acme/service.git

amplihack recipe run default-workflow \
  -c task_description="Add config parser" \
  -c repo_path=.
```

Step 03 emits:

```text
tracking_system=local
tracking_reference=local-482193
tracking_issue=local-482193
issue_creation=local-tracking
issue_number=
```

---

## Regression Contract

Host-aware regression coverage exercises the `workflow-prep` path with shimmed
remotes and provider CLIs.

Required scenarios:

| Scenario | Expected result |
| -------- | --------------- |
| GitHub HTTPS remote | GitHub issue idempotency/create path remains available |
| AzDO `https://dev.azure.com/...` remote | No `gh issue` or `gh label` command is invoked |
| AzDO `https://ORG.visualstudio.com/...` remote | No `gh issue` or `gh label` command is invoked |
| AzDO `git@ssh.dev.azure.com:v3/...` remote | No `gh issue` or `gh label` command is invoked |
| Unsupported remote | Local tracking is used without provider CLI calls; `tracking_reference` / `tracking_issue` propagate to downstream branch, commit, and status steps; local IDs are not coerced into `issue_number` |

The AzDO tests use a `gh` sentinel that fails the test if any GitHub issue or
label command is invoked.
