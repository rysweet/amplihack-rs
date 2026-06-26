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
matching only understands numeric GitHub issue / AzDO work-item ids, so a local
reference must never filter out the legitimate current-work PR (which would
otherwise surface as `no_scoped_pr`).

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
- `tests/integration/default_workflow_terminal_state.rs` proves a local
  tracking `issue_number` does not filter out the current-work PR.
- `tests/gadugi/scenarios/issue-815-804-local-tracking-extract.yaml` exercises
  the same extraction end-to-end via `gadugi-test run`.
