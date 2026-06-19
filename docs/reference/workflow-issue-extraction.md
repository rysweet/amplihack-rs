# Workflow Issue Extraction (Step 03b)

> [Home](../index.md) > Reference > Workflow Issue Extraction

Reference for the tracking extraction logic in `default-workflow` step `03b`.
This step resolves a numeric `issue_number` for GitHub and Azure DevOps, or
preserves a local-prefixed tracking reference for local/unsupported repositories.

## Contents

- [Overview](#overview)
- [Extraction Order](#extraction-order)
  - [Primary source: Step 03 output](#primary-source-step-03-output)
  - [Compatibility fallback: task text references](#compatibility-fallback-task-text-references)
- [Provider Output Formats](#provider-output-formats)
- [Output Contract](#output-contract)
- [Shell Security Constraints](#shell-security-constraints)
- [Configuration](#configuration)
- [Examples](#examples)
- [Troubleshooting](#troubleshooting)
- [Related](#related)

---

## Overview

Step `03b` accepts the `issue_creation` output from step 03 plus the original
`task_description`. For GitHub and Azure DevOps outputs, it produces a
canonical `issue_number` integer. For local tracking outputs, it preserves
`tracking_reference` / `tracking_issue` and leaves `issue_number` empty.

```
issue_creation + task_description ──► 03b extraction ──► issue_number or local reference
```

Step 03 is responsible for emitting a parseable tracking reference. Local
tracking is recognized before any numeric extraction runs, so values such as
`local-123` and `local-tracking:123` remain local identifiers instead of
becoming issue number `123`. If neither Step 03 output nor the compatibility
task-text fallback is parseable, Step 03b fails loudly instead of silently
continuing with the wrong branch or commit reference.

---

## Extraction Order

The sources are evaluated in order; the first match wins.

### Primary source: Step 03 output

Step 03b first parses the `issue_creation` value emitted by Step 03. This is
the canonical source because Step 03 owns host dispatch, provider validation,
reuse decisions, and create/fallback behavior.

Supported `issue_creation` formats:

| Step 03 output | Extraction behavior |
| -------------- | ------------------- |
| `https://github.com/owner/repo/issues/N` | Extracts `N` from `/issues/N` without a network call |
| `https://github.com/owner/repo/pull/N` | Uses `gh pr view` to read the first closing issue; falls back to PR number `N` if no closing issue resolves |
| `https://dev.azure.com/org/project/_workitems/edit/N` | Extracts `N` from `/_workitems/edit/N` without a network call |
| `AB#N` | Extracts `N` without a network call |
| Structured local metadata with `tracking_system=local` plus `tracking_reference=local-*` or `tracking_issue=local-*` | Preserves the local reference; leaves `issue_number` empty |
| Local reference `tracking_reference=local-*` or `tracking_issue=local-*` | Preserves the local reference; leaves `issue_number` empty |
| Legacy `local-tracking:*` | Preserves the local reference; leaves `issue_number` empty |

```
Input:  "https://github.com/rysweet/amplihack-rs/issues/3960"
Match:  issues/3960
Output: issue_number=3960
```

For pull request URLs, Step 03b calls:

```bash
gh pr view <N> \
    --json closingIssuesReferences \
    --jq '.closingIssuesReferences[0].number'
```

The first closing issue reference is used. If the PR closes no issues or the
lookup fails, the PR number itself is used as the numeric tracking ID. Step 03b
does not scan `task_description` for GitHub issue or PR URLs.

```
Input:  "https://github.com/rysweet/amplihack-rs/pull/4143"
gh call: closingIssuesReferences → [3960, 3983]
Output: issue_number=3960   (first reference)
```

**Timeout**: 60 seconds. If `gh` does not respond within the timeout, the PR
number is used as the fallback tracking ID.

---

### Compatibility fallback: task text references

If `issue_creation` is not parseable, Step 03b falls back to references in
`task_description` for compatibility with older or manually supplied contexts:

| Task text pattern | Extraction behavior |
| ----------------- | ------------------- |
| `AB#N` | Extracts `N` |
| `#N` | Extracts `N` |

```
issue_creation:   "unparseable legacy value"
task_description: "Continue work on AB#12345"
Output:           issue_number=12345
```

This fallback is intentionally regex-only and runs only after local tracking has
been ruled out. Step 03 is responsible for deciding whether `#N` means a GitHub
issue or Azure Boards work item; Step 03b preserves the remote numeric workflow
contract without converting local tracking IDs. If input is local but lacks a
supported local reference prefix, Step 03b must fail instead of falling through
to bare `#N` extraction.

---

## Provider Output Formats

Step 03b extracts IDs from the provider-specific output formats produced by
`step-03-create-issue`, plus task-text fallback references used for
compatibility.

| Provider path | Step 03 output | Extraction result |
| ------------- | -------------- | ----------------- |
| GitHub issue | `https://github.com/owner/repo/issues/3960` | `3960` |
| GitHub PR fallback | `https://github.com/owner/repo/pull/4143` | First closing issue, or `4143` if none resolves |
| Azure DevOps work item URL | `https://dev.azure.com/org/project/_workitems/edit/12345` | `12345` |
| Azure Boards shorthand | `AB#12345` | `12345` |
| Local metadata | `tracking_system=local` plus `tracking_reference=local-482193`, `tracking_issue=local-482193`, `issue_creation=local-tracking`, and `issue_number=` | Preserved local reference; empty `issue_number` |
| Compatibility Azure Boards reference | `AB#12345` in `task_description` | `12345` |
| Compatibility bare reference | `#12345` in `task_description` | `12345` |

The `AB#N` shorthand is the canonical reuse output for Azure DevOps runs that
start with an existing `issue_number` context value. It lets Step 03 reuse a
known work item without requiring a live Azure CLI lookup.

---

## Output Contract

After step `03b` completes, the workflow context contains:

| Field | Type | Description |
| ----- | ---- | ----------- |
| `issue_number` | `int` or empty string | Numeric provider ID for GitHub/AzDO, empty for local tracking |
| `tracking_reference` | `string`, optional | Authoritative local tracking reference; required for local success unless legacy `local-tracking:*` is the source |
| `tracking_issue` | `string`, optional | Local tracking issue/reference alias preserved from step 03 |
| `tracking_system` | `string`, optional | `local` when step 03 selected local tracking |

### Example output

```json
{
  "issue_number": 3960
}
```

For local tracking:

```json
{
  "tracking_system": "local",
  "tracking_reference": "local-482193",
  "tracking_issue": "local-482193",
  "issue_number": ""
}
```

If extraction fails, step 03b exits non-zero and prints the unparseable
`issue_creation` value to stderr.

These fields must be declared in the parent workflow context so they propagate
past `workflow-prep`. Downstream steps use `issue_number` for GitHub/Azure
DevOps, and `tracking_reference` / `tracking_issue` for local branch names,
commit messages, publish output, and final status.

### Local detection contract

`tracking_system=local` is only a mode marker. It is not sufficient on its own.
Local extraction succeeds only when one of these local references is present:

- `tracking_reference=local-*`
- `tracking_issue=local-*`
- legacy `local-tracking:*`

The `local-*` prefix is intentional. It prevents unrelated numeric text in local
metadata from being treated as a GitHub issue or Azure Boards work item.

---

## Shell Security Constraints

Step `03b` follows the same shell-security rules as the rest of the default
workflow:

| Constraint | Detail |
| ---------- | ------ |
| **Quoted variables** | `ISSUE_CREATION` and `TASK_DESCRIPTION` are assigned and matched with quoted shell variables |
| **Regex-only numeric capture** | Every accepted identifier is captured with `[0-9]+`; no provider CLI receives an unvalidated ID |
| **60-second timeout** | GitHub PR closing-issue lookup uses `timeout 60`; hung subprocesses do not stall the workflow |
| **No provider crossover** | Azure Boards formats (`AB#N`, `_workitems/edit/N`) are parsed locally and do not require GitHub CLI calls |
| **Local before numeric** | Local metadata is detected before numeric regex extraction so `local-123` and `local-tracking:123` are not coerced into issue `123` |
| **Visible failure** | If no supported pattern matches, the step exits non-zero and prints the unparseable `issue_creation` value to stderr |

---

## Configuration

Step `03b` reads the following values from the workflow context:

| Context key | Required | Description |
| ----------- | -------- | ----------- |
| `issue_creation` | Yes | Step 03 output: GitHub URL, Azure Boards URL, `AB#N`, structured local metadata with a local-prefixed reference, or legacy `local-tracking:*` |
| `task_description` | Yes | Free-form string used only as a compatibility fallback source for `AB#N` or `#N` references |

No environment variables are specific to step `03b`. GitHub PR closing-issue
lookup relies on the authenticated `gh` CLI configured in the user's shell.
Azure Boards extraction is regex-only. Local tracking preservation is prefix and
metadata based.

---

## Examples

### Resolving from an issue URL

```yaml
# Recipe context snippet
issue_creation: "https://github.com/rysweet/amplihack-rs/issues/3960"
```

```
issues/ match: 3960
issue_number: 3960
```

---

### Resolving from a PR URL

```yaml
issue_creation: "https://github.com/rysweet/amplihack-rs/pull/4143"
```

```
gh pr view 4143 --json closingIssuesReferences
→ [3960, 3983]
issue_number: 3960
```

---

### Resolving from compatibility task text fallback

```yaml
issue_creation: "legacy output"
task_description: "Continue work on #3983"
```

```
No supported issue_creation format → task text fallback
issue_number: 3983
```

---

### Resolving from an Azure Boards shorthand

```yaml
issue_creation: "AB#12345"
task_description: "Address review feedback for the Azure DevOps PR"
```

```
AB# match: 12345
issue_number: 12345
```

---

### Resolving from local tracking

```yaml
issue_creation: |
  tracking_system=local
  tracking_reference=local-482193
  tracking_issue=local-482193
  issue_creation=local-tracking
  issue_number=
```

```
local tracking match: tracking_reference=local-482193
tracking_reference: local-482193
issue_number: ""
```

---

### Resolving from legacy local tracking

```yaml
issue_creation: "local-tracking:123"
task_description: "Add config parser"
```

```
legacy local tracking match: local-tracking:123
tracking_reference: local-tracking:123
issue_number: ""
```

The numeric suffix is preserved as part of the local reference. It is not copied
to `issue_number`.

---

### Invalid step 03 output

```yaml
issue_creation: "unparseable provider output"
```

```
No supported pattern → step 03b exits non-zero
stderr includes the unparseable issue_creation value
```

---

## Troubleshooting

**`gh: command not found`**

Install and authenticate the GitHub CLI: `gh auth login`.
Only GitHub PR closing-issue lookup requires `gh`; direct issue URLs, Azure
Boards outputs, `AB#N`, structured local metadata, and legacy
`local-tracking:*` still resolve without it.

**GitHub PR lookup returns no closing issues**

The PR may not use closing keywords (`Closes #N`, `Fixes #N`). In that case
Step 03b uses the PR number itself as the numeric tracking ID.

**Task text fallback resolves a bare `#N` unexpectedly**

Step 03b does not verify bare `#N` against a provider. Provider validation must
happen in Step 03 before it emits `issue_creation`.

**Task text fallback does not resolve a `#N` that is present**

The `#N` pattern requires at least one digit. Values like `#` alone or
`#abc` are not matched.

---

## Multi-Provider Extraction

Step 03b extracts issue numbers from provider-specific URL formats in addition
to GitHub issue and PR patterns:

| Provider | Accepted pattern | Extraction regex |
| -------- | ---------------- | ---------------- |
| GitHub | `https://github.com/owner/repo/issues/N` | `issues/([0-9]+)` |
| GitHub PR | `https://github.com/owner/repo/pull/N` | `pull/([0-9]+)` plus closing-issue lookup |
| Azure DevOps | `https://dev.azure.com/org/project/_workitems/edit/N` | `_workitems/edit/([0-9]+)` |
| Azure DevOps | `AB#N` | `AB#([0-9]+)` |
| Local | Structured metadata with `tracking_system=local` plus `tracking_reference=local-*` or `tracking_issue=local-*` | Preserve the local reference; leave `issue_number` empty |
| Local | `tracking_reference=local-*` or `tracking_issue=local-*` | Preserve the local reference; leave `issue_number` empty |
| Local legacy | `local-tracking:*` | Preserve the local reference; leave `issue_number` empty |

The remote `issue_number` output contract is unchanged: GitHub and Azure DevOps
produce plain integers. Local tracking intentionally does not. See
[Multi-Provider Workflow Reference](multi-provider-workflow.md) for full
details.

---

## Related

- [Workflow Execution Guardrails Reference](recipe-quick-reference.md)
  — canonical execution root and GitHub identity gate (also part of the default
  workflow)
- [How to Configure Workflow Execution Guardrails](../howto/configure-hooks.md)
- [Default Workflow](../concepts/default-workflow.md) — full step list
- [Lock Session ID Sanitization](security-recommendations.md)
  — the security fix delivered alongside this extraction improvement (PR #4143)
- [Multi-Provider Workflow Reference](multi-provider-workflow.md) — provider detection and routing
- Issues [#3960](https://github.com/rysweet/amplihack-rs/issues/3960) and
  [#3983](https://github.com/rysweet/amplihack-rs/issues/3983) — originating work
- PR [#4143](https://github.com/rysweet/amplihack-rs/pull/4143) — implementation

---

**Metadata**

| Field       | Value                                                         |
| ----------- | ------------------------------------------------------------- |
| Status      | Issue #718 target contract                                    |
| Issues      | #3960, #3983, #718                                            |
| PR          | #4143                                                         |
| Recipe file | `amplifier-bundle/recipes/default-workflow.yaml` (step `03b`) |
| Test file   | `amplifier-bundle/tools/test_default_workflow_fixes.py`       |
| Gadugi spec | `tests/gadugi/lock-recovery-and-issue-extraction.yaml`        |
