# Provider-Aware Workflow Tracking

**Host-aware issue tracking for `workflow-prep` and `default-workflow`.**

> [Home](../index.md) > [Features](README.md) > Provider-Aware Workflow Tracking

## Quick Navigation

- [How to configure provider-aware workflow tracking](../howto/configure-dual-provider-workflow.md)
- [How to use the workflow with Azure DevOps](../howto/use-workflow-with-azure-devops.md)
- [Tutorial: running the workflow on an Azure DevOps repository](../tutorials/dual-provider-workflow.md)
- [Provider-aware workflow reference](../reference/dual-provider-workflow.md)
- [Multi-provider workflow reference](../reference/multi-provider-workflow.md)

---

## What This Feature Does

`workflow-prep` classifies the repository's `origin` remote before it tries to
create or reuse a tracking item. The classification is stored as
`REMOTE_HOST_TYPE` and is one of:

| Host type | Repository remote | Tracking behavior |
| --------- | ----------------- | ----------------- |
| `github` | GitHub HTTPS or SSH remote | Use GitHub Issues through `gh issue` and GitHub labels through `gh label` |
| `azdo` | Azure DevOps HTTPS, legacy `visualstudio.com`, or SSH remote | Use the Azure Boards/local-metadata path; never call GitHub issue or label commands |
| `other` | Missing, local, malformed, GitLab, Bitbucket, or unsupported remote | Use structured local metadata; never call provider CLIs |

The key guarantee is provider isolation: **GitHub issue and label commands run
only when `REMOTE_HOST_TYPE=github`**. Azure DevOps and other non-GitHub
repositories never attempt `gh issue view`, `gh issue list`, `gh issue create`,
or `gh label` setup.

---

## Supported Remote Forms

The finished host detector must normalize the `origin` URL and match the
remote host, not an arbitrary substring in a spoofable path.

| Remote URL | Host type |
| ---------- | --------- |
| `https://github.com/OWNER/REPO.git` | `github` |
| `git@github.com:OWNER/REPO.git` | `github` |
| `ssh://git@github.com/OWNER/REPO.git` | `github` |
| `https://dev.azure.com/ORG/PROJECT/_git/REPO` | `azdo` |
| `https://ORG.visualstudio.com/PROJECT/_git/REPO` | `azdo` |
| `git@ssh.dev.azure.com:v3/ORG/PROJECT/REPO` | `azdo` |
| `ssh://git@ssh.dev.azure.com/v3/ORG/PROJECT/REPO` | `azdo` |
| `https://gitlab.com/OWNER/REPO.git` | `other` |
| `https://github.com.evil.example/OWNER/REPO.git` | `other` |
| No `origin` remote | `other` |

---

## Workflow Prep Routing

`workflow-prep.yaml` performs provider dispatch before any provider command is
run:

```text
step-02d-detect-host-type
        |
        v
REMOTE_HOST_TYPE = github | azdo | other
        |
        v
step-03-create-issue
        |
        +-- github -> gh issue view/list/create + gh label setup
        +-- azdo  -> Azure Boards reuse/create, then local metadata if unavailable
        +-- other -> structured local metadata
```

GitHub repositories keep the existing idempotent issue behavior: reuse a
referenced issue, search for an open issue with a matching title, then create a
new issue when needed.

Azure DevOps repositories stay on the Azure Boards/local-metadata path. When an
existing work item is supplied with `issue_number=N` or `AB#N`, the workflow
reuses it. When Azure Boards is unavailable or cannot create a work item, the
workflow emits structured local metadata instead of falling into GitHub logic.

Other repositories always use structured local metadata.

Local metadata is multiline step output, not a provider URL:

```text
tracking_system=local
tracking_reference=local-482193
tracking_issue=local-482193
issue_creation=local-tracking
issue_number=
```

For local tracking, `tracking_reference` and `tracking_issue` are the
authoritative identifiers. `tracking_system=local` marks local mode, but Step
03b succeeds only when a local-prefixed `tracking_reference` /
`tracking_issue` or legacy `local-tracking:*` reference is present. Step 03b
checks those references before numeric extraction, preserves the local
reference, and leaves `issue_number` empty. Local IDs are never coerced into
GitHub-style issue numbers.

---

## Configuration Summary

No provider flag is required for normal use. The workflow reads
`git remote get-url origin` and routes automatically.

| Repository type | Required tools |
| --------------- | -------------- |
| GitHub | `git`, authenticated `gh` |
| Azure DevOps with Boards tracking | `git`, `az`, Azure DevOps extension, Azure Boards permissions |
| Azure DevOps with local fallback | `git` |
| Other/local | `git` |

For large nested workflow runs, keep the project preference:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## Examples

### GitHub repository

```bash
git remote set-url origin https://github.com/acme/service.git

amplihack recipe run default-workflow \
  -c task_description="Fix login timeout #684" \
  -c repo_path=.
```

Expected routing:

```text
REMOTE_HOST_TYPE=github
step-03-create-issue -> gh issue view/list/create
```

### Azure DevOps repository

```bash
git remote set-url origin https://dev.azure.com/acme/platform/_git/service

amplihack recipe run default-workflow \
  -c task_description="Fix login timeout in AB#12345" \
  -c repo_path=.
```

Expected routing:

```text
REMOTE_HOST_TYPE=azdo
step-03-create-issue -> Azure Boards/local tracking
gh issue commands -> not invoked
gh label commands -> not invoked
```

### Unsupported remote

```bash
git remote set-url origin https://gitlab.com/acme/service.git

amplihack recipe run default-workflow \
  -c task_description="Add config parser" \
  -c repo_path=.
```

Expected routing:

```text
REMOTE_HOST_TYPE=other
step-03-create-issue -> structured local metadata
provider CLIs -> not invoked
```

---

## See Also

- [Provider-aware workflow reference](../reference/dual-provider-workflow.md)
- [Step 03 Host-Aware Tracking Idempotency](../reference/recipe-step-03-idempotency.md)
- [Workflow Issue Extraction Reference](../reference/workflow-issue-extraction.md)
- [Multi-Provider Workflow Architecture](../concepts/multi-provider-workflow-architecture.md)
