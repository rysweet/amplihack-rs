# Multi-Provider Workflow Reference

> [Home](../index.md) > Reference > Multi-Provider Workflow

> [PLANNED - Implementation Pending]
>
> This page is the target implementation reference for provider-neutral workflow
> helpers. `amplihack workflow ...` commands are planned helper commands until
> the feature implementation lands.

`default-workflow` is provider-neutral. It supports GitHub, Azure DevOps, local,
and unsupported repositories through typed workflow helper commands and provider
adapters. Recipes do not inline provider parsing or infer success from shell
output.

## Contents

- [Overview](#overview)
- [Provider detection](#provider-detection)
- [Tracking items](#tracking-items)
- [Change requests](#change-requests)
- [Terminal state](#terminal-state)
- [Recipe contract](#recipe-contract)
- [Examples](#examples)
- [Regression contract](#regression-contract)
- [Related documentation](#related-documentation)

## Overview

The workflow classifies the repository provider once, persists the provider
context, and routes later provider operations through helper commands:

```text
amplihack workflow detect-provider
        |
        v
ProviderContext { provider, repository, capabilities }
        |
        +-- tracking-item ensure
        +-- change-request publish
        +-- change-request status
        +-- cleanup-stale
        +-- terminal-state
```

Every helper emits JSON. Recipes validate the JSON schema before using it.
Provider-specific tools such as `gh` and `az` run only inside the matching
adapter.

## Provider detection

```bash
amplihack workflow detect-provider \
  --repo . \
  --format json
```

Example output for GitHub:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "DetectProvider",
  "status": "Succeeded",
  "next_action": "No further provider setup is required.",
  "warnings": [],
  "data": {
    "repository": {
      "remote_url": "https://github.com/acme/service.git",
      "owner": "acme",
      "name": "service",
      "default_base": "main"
    },
    "capabilities": {
      "tracking_items": "Automated",
      "change_requests": "Automated",
      "stale_cleanup": "Automated"
    }
  }
}
```

Supported provider classifications:

| Provider | Remote examples |
| --- | --- |
| `GitHub` | `https://github.com/OWNER/REPO.git`, `git@github.com:OWNER/REPO.git`, `ssh://git@github.com/OWNER/REPO.git` |
| `AzureDevOps` | `https://dev.azure.com/ORG/PROJECT/_git/REPO`, `https://ORG.visualstudio.com/PROJECT/_git/REPO`, `git@ssh.dev.azure.com:v3/ORG/PROJECT/REPO` |
| `Local` | No `origin` remote or local-only repository |
| `Unsupported` | Any recognized Git repository whose provider has no adapter |

Classification matches host components, not arbitrary substrings. A remote such
as `https://github.com.evil.example/acme/service.git` is `Unsupported`, not
`GitHub`.

## Tracking items

```bash
amplihack workflow tracking-item ensure \
  --repo . \
  --title "Fix authentication timeout" \
  --body-file /tmp/workflow-body.md \
  --format json
```

Canonical result:

```json
{
  "schema_version": 1,
  "provider": "AzureDevOps",
  "operation": "EnsureTrackingItem",
  "status": "Succeeded",
  "next_action": "Use AB#12345 in commits and change-request descriptions.",
  "warnings": [],
  "data": {
    "tracking_item": {
      "kind": "WorkItem",
      "id": "12345",
      "display_ref": "AB#12345",
      "url": "https://dev.azure.com/acme/platform/_workitems/edit/12345"
    }
  }
}
```

Provider behavior:

| Provider | Behavior |
| --- | --- |
| GitHub | Reuse or create a GitHub issue through `gh issue`. |
| Azure DevOps | Reuse or create an Azure Boards work item when `az` is configured; otherwise return local tracking or `BlockedManualProvider` according to the requested strictness. |
| Local | Create a local workflow reference such as `local-550e8400`. |
| Unsupported | Return `ManualRequired` with a provider-neutral next action. |

Recipes consume `tracking_item.display_ref`, not provider-specific issue syntax.
Commit formatting is delegated to the helper output.

## Change requests

```bash
amplihack workflow change-request publish \
  --repo . \
  --source-branch feat/auth-timeout \
  --base main \
  --title "Fix authentication timeout" \
  --body-file /tmp/pr-body.md \
  --format json
```

GitHub automated result:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "PublishChangeRequest",
  "status": "Succeeded",
  "next_action": "Wait for required checks and review.",
  "warnings": [],
  "data": {
    "change_request": {
      "kind": "PullRequest",
      "id": "812",
      "url": "https://github.com/acme/service/pull/812",
      "state": "Open",
      "source_branch": "feat/auth-timeout",
      "base_branch": "main"
    }
  }
}
```

Azure DevOps manual result:

```json
{
  "schema_version": 1,
  "provider": "AzureDevOps",
  "operation": "PublishChangeRequest",
  "status": "ManualRequired",
  "next_action": "Create an Azure Repos pull request from feat/auth-timeout to main and include AB#12345 in the description.",
  "warnings": [],
  "data": {
    "change_request": null,
    "manual_action": {
      "kind": "CreateChangeRequest",
      "source_branch": "feat/auth-timeout",
      "base_branch": "main",
      "title": "Fix authentication timeout",
      "body_summary": "Includes AB#12345 and validation evidence."
    }
  }
}
```

Manual states are explicit workflow output. They are never converted into
successful publication evidence unless a later provider status query proves a
matching change request exists.

## Terminal state

Provider-aware finalization uses the shared terminal-state contract:

```bash
amplihack workflow terminal-state \
  --repo . \
  --branch feat/auth-timeout \
  --base main \
  --format json
```

Terminal-state helpers combine local Git evidence, provider-safe change-request
metadata, CI state, implementation markers, verification markers, and agentic
finalizer output. They emit exactly one terminal state.

Provider-specific manual states:

| State | Success? | Meaning |
| --- | --- | --- |
| `MANUAL_REQUIRED` | No | Work is ready for a provider action that amplihack does not automate. |
| `BLOCKED_MANUAL_PROVIDER` | No | Required provider tooling, auth, permissions, or APIs are unavailable. |

Both states require `next_action`. They keep the final state auditable instead
of claiming success where automation did not run.

## Recipe contract

Recipes use deterministic helpers for stable logic and agent steps for
adaptable judgment:

| Recipe concern | Owner |
| --- | --- |
| Provider classification | `amplihack workflow detect-provider` |
| Tracking-item parsing and creation | `amplihack workflow tracking-item ensure` |
| Change-request publication and status | `amplihack workflow change-request ...` |
| Stale/superseded cleanup decisions | Agentic classifier plus `validate-agent-contract`; provider helper applies or dry-runs the result. |
| Final terminal assessment | Agentic finalizer plus deterministic `terminal-state` validation. |
| JSON schema validation | `amplihack workflow validate-agent-contract` or helper-specific validation. |

Recipes may use shell for step sequencing, file movement, and invoking helpers.
They must not add new brittle parsing steps for provider routing, PR status,
terminal state, or agentic decisions.

## Examples

### GitHub repository

```bash
git remote set-url origin https://github.com/acme/service.git

amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout" \
  -c repo_path=. \
  --format json
```

Expected provider behavior:

```text
provider=GitHub
tracking_items=Automated
change_requests=Automated
stale_cleanup=Automated
```

### Azure DevOps repository

```bash
git remote set-url origin https://dev.azure.com/acme/platform/_git/service

amplihack recipe run default-workflow \
  -c task_description="Fix authentication timeout in AB#12345" \
  -c repo_path=. \
  --format json
```

Expected provider behavior:

```text
provider=AzureDevOps
tracking_items=Automated when Azure Boards is configured
change_requests=Automated when Azure Repos is configured
GitHub commands=not invoked
```

### Local repository

```bash
git remote remove origin

amplihack recipe run default-workflow \
  -c task_description="Add config parser" \
  -c repo_path=. \
  --format json
```

Expected provider behavior:

```text
provider=Local
tracking_items=local reference
change_requests=ManualRequired
remote provider commands=not invoked
```

## Regression contract

Regression coverage for this feature verifies:

1. GitHub, Azure DevOps, local, unsupported, and spoofed remotes classify
   correctly.
2. Provider-specific commands run only inside the matching adapter.
3. Deterministic parsing lives in typed helpers and has unit coverage.
4. Agentic finalization and stale/superseded decisions fail closed when their
   JSON contract is missing, malformed, low-confidence, or unsupported.
5. Azure DevOps and local publication paths return `ManualRequired` or
   `BlockedManualProvider` with actionable `next_action` text when automation is
   unavailable.
6. Recipe simulation tests cover representative success, failure, manual, and
   blocked paths without live GitHub or Azure DevOps calls.

## Related documentation

- [Provider-Neutral Workflow Architecture](../concepts/multi-provider-workflow-architecture.md)
- [Provider-Neutral Workflow API](workflow-provider-contract.md)
- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)
- [Recipe Simulation Reference](workflow-simulation.md)
