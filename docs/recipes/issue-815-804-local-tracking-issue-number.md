# Local Tracking Issue-Number Extraction (Issues #815, #804)

`default-workflow` no longer hard-fails — and no longer silently drops the
tracking reference — when the workflow falls back to **local tracking** and
produces a hash-based reference such as `local-5d904cff4398` instead of a
numeric GitHub issue or Azure DevOps work-item id.

---

## Problem

When issue creation is unavailable (no GitHub/AzDO access, permissions, offline
runs), `workflow-prep` `step-03-create-issue` falls back to local tracking and
emits metadata like:

```text
tracking_system=local
tracking_reference=local-5d904cff4398
tracking_issue=local-5d904cff4398
issue_creation=local-tracking
```

`step-03b-extract-issue-number` then tried to extract a **numeric** issue
number from this output. Because the local reference is non-numeric, the step
aborted the whole workflow *after* classification, analysis, and ambiguity
resolution had already completed:

```text
ERROR: step-03b failed to extract issue number from issue_creation output.
    tracking_system=local
    tracking_reference=local-5d904cff4398
    issue_creation=local-tracking
```

This was deterministic for local-tracking mode and defeated the purpose of the
enforced default workflow.

## Solution

### 1. `step-03b-extract-issue-number` propagates the local reference

The step now branches on local-tracking metadata and **propagates the
well-formed local reference verbatim** (`local-<hash>`, `local-issue-<n>`,
legacy `local-tracking:<n>`) as the `issue_number` output, instead of dropping
it to an empty string. It still:

- extracts real **numeric** ids for GitHub issues/PRs and AzDO work items,
- **never surfaces a bare embedded number** for a local fallback (e.g.
  `local-issue-763` yields `local-issue-763`, never `763`, so it can never
  become a `Closes #763` against an unrelated issue), and
- **fails closed** (with credential sanitization) for genuinely unparseable
  output and for malformed local metadata that carries markers but no usable
  reference.

This satisfies issue #815's preferred fix: *"emit the local tracking id as a
valid tracking identifier without numeric parsing."*

### 2. Terminal-state PR scoping ignores non-numeric ids

Because `issue_number` can now legitimately carry a non-numeric local
reference, `workflow-terminal-state` coerces a non-numeric value to empty
before passing `--issue` / `--work-item` to `workflow_pr_scope.sh`. PR-scope
matching resolves the current-work PR by its primary key
(`repository + head + base`, same-repo), and `--issue` / `--work-item` act only
as tie-breakers among multiple candidates — so a local reference can never
filter out the legitimate current-work PR. Coercing the non-numeric value to
empty keeps the tie-breaker inputs clean and avoids passing a value the matcher
does not interpret as a GitHub issue / AzDO work-item id.

### 3. Step-16 publishes a local reference as "no linked issue" (#1976)

`workflow_publish_pr.sh`, the step-16 PR creation tool, classifies
`issue_number` once before it builds the PR:

| `issue_number` | Class | What step-16 does |
| --- | --- | --- |
| All digits, such as `123` | linked issue | Unchanged: `(#123)` title suffix, the `## Issue` section from `workflow_issue_reference.sh`, the claim check, and `AB#123` on Azure DevOps. |
| Empty | no linked issue | See below. |
| `local-` followed by 1 to 128 of `A-Z a-z 0-9 . _ : -`, such as `local-5d904cff4398`, `local-issue-763` or `local-tracking:123` | no linked issue | See below. |
| Anything else | invalid | Stops with publish status `FAILED_INVALID_INPUT` and exit `1`. No PR is created. |

A local reference matches the anchored pattern `^local-[A-Za-z0-9._:-]+$`,
with at most 128 characters after `local-`. This is the same character set
`step-03b-extract-issue-number` accepts in `workflow-prep.yaml`, so every
reference step-03b passes through is also accepted at step-16. That covers the
formats `derive_local_tracking_id` in `workflow_issue_tracking.sh` produces
(`local-<hash>`, `local-issue-<n>`, `local-ab-<n>`), the legacy
`local-tracking:<n>` form and any other caller-supplied `local-` reference.
The pattern excludes `#`, whitespace, backticks and control characters, so the
value is safe to put in the PR body.

With no linked issue, step-16:

- creates the PR with no `(#N)` suffix on the title;
- writes the `## Issue` section as
  ``No linked issue (local tracking reference `local-5d904cff4398`).``, or
  `No linked issue.` when the value is empty;
- does not call `workflow_issue_reference.sh`;
- skips the issue claim check;
- writes no `AB#` reference on Azure DevOps.

The title and body never contain `#local` or `Closes`, so a local reference
can never close an unrelated issue.

An invalid value is reported through `finish_publish`, which prints the
publish result and exits `1`:

```json
{"state":"FAILED_INVALID_INPUT","legacy_state":"invalid-input","terminal_status":"failure","pr_url":"","pr_number":"","branch_diff_status":"","message":"issue_number 'garbage' is neither numeric nor a local tracking id"}
```

The value in the reason is cut to 64 characters, has control characters
removed and is JSON-encoded, so a crafted `issue_number` cannot inject text
into the status record.

## Behavior summary

| `issue_creation` payload                                   | `issue_number` output |
| ---------------------------------------------------------- | --------------------- |
| `tracking_reference=local-5d904cff4398` (local fallback)   | `local-5d904cff4398`  |
| `tracking_reference=local-issue-763` + `issue_number=763`  | `local-issue-763`     |
| `tracking_reference=local-tracking:123` (legacy)           | `local-tracking:123`  |
| `https://github.com/org/repo/issues/901`                   | `901`                 |
| `https://dev.azure.com/org/proj/_workitems/edit/4242`      | `4242`                |
| local markers present, no valid reference                  | fails closed (exit 1) |
| unparseable, non-local output                              | fails closed (exit 1) |

## Verification

- `tests/integration/issue_815_804_local_tracking_extract_test.rs` and the
  local-tracking cases in `issue_684_host_aware_workflow_test.rs` and
  `default_workflow_decomposition_test.rs` execute the **shipped** recipe bash
  body for every row above.
- `tests/integration/issue_1976_publish_local_issue.rs` runs step-16 with a
  fake `gh` and a bare-repo remote for `local-abc123def456`,
  `local-tracking:123`, `garbage` and `123`.
- `tests/integration/default_workflow_terminal_state.rs` proves a local
  tracking `issue_number` does not filter out the current-work PR.
- `tests/gadugi/scenarios/issue-815-804-local-tracking-extract.yaml` exercises
  the same extraction end-to-end via `gadugi-test run`.
