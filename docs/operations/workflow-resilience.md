---
title: Workflow Publish and Finalize Resilience
description: Publish and finalize outcomes for existing pull requests and terminal branch states.
doc_type: reference
---

# Workflow Publish and Finalize Resilience

> Compatibility note: this page now uses the provider-neutral workflow contract.
> GitHub PR publication can be automated. Azure DevOps, local, and unsupported
> change-request publication return explicit manual or blocked provider states
> instead of success-shaped `non-github` output.

`workflow-publish` and `workflow-finalize` classify already-terminal states
before taking PR actions. Re-running a workflow after a merge, after an
already-created PR, or on a branch with no diff produces a successful terminal
outcome instead of creating duplicate PRs or failing on already-finished work.

Finalization is agentic only where judgment is useful, and that judgment never
drives control flow. Deterministic steps collect Git/PR/CI/implementation/
verification evidence into typed recipe state, classify the terminal state from
that typed evidence, persist normalized terminal fields, and return the process
exit code. The agentic finalizer contributes a human-readable narrative artifact
only; it is never parsed. A reporting step that fails after a successful
implementation classifies as `FAILED_REPORTING` with durable evidence preserved,
and malformed typed evidence no longer ends the run: finalization classifies on the pushed head and open PR (#2343). `FAILED_INVALID_EVIDENCE` is still recognised in the vocabulary but is no longer emitted by finalization.

## Observed failure modes

The resilience model exists because recent workflow runs and regression tests
exposed these concrete failure modes:

| Failure mode | Typical symptom | Resilient behavior |
| --- | --- | --- |
| Brittle parsing | A shell step inferred success from a PR URL, status string, or partial command output. | Only structured JSON/key-value evidence can prove terminal success. |
| Missing or stale PR metadata | `pr_number` or `pr_url` points to the wrong branch, stale head SHA, closed PR, or unavailable provider metadata. | PR identity must match repo, branch, base, and head SHA before PR state is trusted. |
| Dirty worktree misclassification | Generated artifacts or unstaged edits are treated as harmless no-diff work. | Dirty worktree blocks success. At finalization it first goes to a fresh assessment session with the entries as evidence (#2966); a tree still dirty after that ends with `FAILED_DIRTY_WORKTREE`, naming the entries. |
| Closed-unmerged PR handling | A closed PR without merge evidence is treated as completed while branch diff remains. | Finalization returns `FAILED_CLOSED_UNMERGED` unless local obsolete/no-diff proof supports `CLOSED_OBSOLETE`. |
| Remaining meaningful diff | Branch changes remain but no valid PR, merge, follow-up, or verified implementation path proves closure. | Finalization returns `FAILED_MEANINGFUL_DIFF` rather than treating the branch as a no-op success. |
| Missing tooling | Required `git`, `jq`, provider CLI, or provider auth is unavailable on a path that depends on it. | Finalization returns `FAILED_MISSING_TOOLING`, `FAILED_PR_METADATA_UNAVAILABLE`, or `BLOCKED_MANUAL_PROVIDER`; it does not silently skip required proof. |
| Failed CI | Open PR has failing checks but final output looks complete. | Finalization repairs it: diagnose, fix, push, wait for CI and reassess until the checks are green, with no round or time limit (#2966). It returns `BLOCKED_CI` and a nonzero exit only for a cause the run cannot fix, with the check, the cause, the owner and the exact step named. |
| Hollow success | Workflow exits after setup, design, empty agent output, or inaccessible-codebase messages. | Finalization returns `HOLLOW_SUCCESS`, which #2343 leaves unchanged. Where the agent output was not empty, the run is classified on its real outcome: no pushed head or open pull request is a failure naming that gap. |

## Publish behavior

Before creating a pull request, `workflow-publish` must classify the repository and
branch state.

| State | Publish result |
| --- | --- |
| Azure DevOps host | `ManualRequired` with an Azure Repos pull-request action; no `gh pr create` or `az repos pr create` call. |
| Local or unsupported host | `ManualRequired` with provider-neutral next action; no remote provider command. |
| No branch diff against base | Success with `state=no-diff`; no PR is created. |
| Existing open PR for the same head branch | Success with `state=existing-open-pr`; existing PR URL is returned. On GitHub with `base_branch` empty, an open PR into another base counts too; its base is kept, and it takes precedence over a merged or closed PR into the default branch ([details](../reference/workflow-delivery-inputs.md#an-open-pull-request-on-another-base)). |
| Existing merged PR for the same head branch | Success with `state=already-merged`; merged PR URL is returned. |
| Existing closed PR that was merged | Success with `state=closed-after-merge`; merged PR URL is returned. |
| Closed, unmerged PR selected after open-PR discovery, with branch diff | Failure with a clear action message. |
| Branch has diff and no existing PR | Create one draft PR and return its URL. |

The recipe checks branch diff and PR state before publishing and never creates
an empty PR. On GitHub with `base_branch` empty, it prefers an eligible open PR
on the resolved base, otherwise adopts a unique eligible open PR on another
base. It records that PR's number, URL and actual base. Multiple eligible
cross-base candidates stop publication before creation. An explicit
`base_branch` keeps lookup and creation scoped to that base.

Publishing is also collision-tolerant at create time. If `gh pr create` fails
because a pull request already exists for the branch (a race with the pre-create
check, or a stale local view), `workflow-publish` re-runs the scoped PR lookup;
when an OPEN PR now exists it returns `state=existing-open-pr` success rather than
`FAILED_PR_CREATE`. Collision recovery uses the same base-selection rules as
the initial lookup. If it cannot identify an eligible open PR, including when
the lookup is ambiguous, publication returns `FAILED_PR_CREATE`.

When a closed-unmerged PR is selected after open-PR discovery, remaining branch
diff is a hard failure requiring user action. The workflow must not silently
republish or mark that PR complete. With `base_branch` empty, an eligible open
cross-base PR takes precedence over a closed-unmerged PR on the resolved base.

## Finalize behavior

`workflow-finalize` uses the same terminal-state model and the structured
agentic finalizer described in
[Default Workflow Agentic Finalization](../reference/default-workflow-agentic-finalization.md).

| State | Finalize result |
| --- | --- |
| No diff and no PR required | Success with `terminal_state=NO_DIFF_SUCCESS`. |
| PR already merged | Success with `terminal_state=MERGED`. |
| PR closed after merge | Success with `terminal_state=MERGED` when merge evidence exists. |
| PR or branch obsolete | Success with `terminal_state=CLOSED_OBSOLETE` only when local no-diff/obsolete proof exists. |
| Run superseded by a newer workflow-owned PR or issue | Success with `terminal_state=SUPERSEDED` only when durable replacement metadata and reason text are present. |
| Open PR with green checks | Success with `terminal_state=EXISTING_OPEN_PR`; merge according to repository policy. |
| Open PR with pending checks | The assessment waits for them with `gh pr checks --watch`, then decides on what they report. Pending is not success. |
| Open PR with failing checks the run can fix | Repaired in a loop with no round or time limit; success once the head is green. CI gating remains active and nothing is skipped to get past it. |
| Open PR with a failing check the run cannot fix (missing secret, run awaiting maintainer approval, owner-only setting, account or billing block), or whose fix needs an edit under `.github/**`, which this workflow reserves to its owner, found after diagnosis | Failure with `terminal_state=BLOCKED_CI`, the check and cause as the reason (an owner decision says so), and the owner and exact step as the next action. The run exits `81` after step recovery hands it off. |
| Closed unmerged PR with remaining branch diff | Failure with `terminal_state=FAILED_CLOSED_UNMERGED`; user action is required. |
| Meaningful branch diff without terminal publication or verification proof | Failure with `terminal_state=FAILED_MEANINGFUL_DIFF`. |
| Missing or ambiguous PR state | Failure; ambiguity is not treated as success. |
| Implementation or verification evidence absent while work remains | Failure with `terminal_state=FAILED_IMPLEMENTATION`. |
| Reporting step fails after successful implementation | Failure with `terminal_state=FAILED_REPORTING`; durable `pr_url`/`pr_number` and implementation/verification evidence are preserved. |
| Success-looking run without completion evidence | Failure with a gap reported from the pushed head and PR facts, normally `INCOMPLETE` (#2343). |

Already-terminal states are success only when verified through Git branch diff,
GitHub PR metadata, or both. Failing checks on an active PR are repaired and
pending checks are waited on; terminal-state resilience must not bypass CI, and
the repair never lowers the bar (no skip directive, no weakened test, no
non-blocking check). See
[Red CI at Finalization](../reference/default-workflow-agentic-finalization.md#red-ci-at-finalization-2966).

## Recipe context reference

These context keys influence publish/finalize behavior:

| Context key | Used by | Meaning |
| --- | --- | --- |
| `repo_path` | publish, finalize | Repository root to inspect. Defaults to the recipe working directory. |
| `provider_context` | publish, finalize | Structured provider helper result. `GitHub` enables GitHub PR handling; `AzureDevOps`, `Local`, and `Unsupported` use manual or blocked publication states. |
| `remote_host_type` | publish | Legacy compatibility input. Implementations may accept `github`, `azure-devops`, `azdo`, or `local`, but must normalize to `provider_context` before provider operations. |
| `branch_name` | publish, finalize | Current workflow branch or explicit branch to inspect. |
| `base_branch` | publish, finalize | Explicit base for lookup, creation and diff checks. When empty, publish resolves the default base and can adopt an open GitHub PR on another base; finalize uses the recorded PR base. See [base selection](../reference/workflow-delivery-inputs.md#an-open-pull-request-on-another-base). |
| `pr_number` | finalize | Existing PR to finalize when known. |
| `pr_url` | finalize | Existing PR URL to parse when `pr_number` is absent. |
| `issue_number` | publish | Issue/work item identifier used in PR title/body generation. |
| `task_description` | publish | Human-readable task text used in PR title/body generation. |

Recipe outputs include the publish result plus one canonical finalization result
object. In full recipe JSON, the finalization result is stored under
`workflow_result`; shell steps may also expose the same fields as flattened
key/value output.

| Output key | Meaning |
| --- | --- |
| `pr_publish_result.pr_url` | Created or reused PR URL, when a GitHub PR exists. |
| `pr_publish_result.pr_number` | Created or reused PR number, when available. |
| `pr_publish_result.state` | Publish classifier result. |
| `workflow_result.terminal_success` | Whether finalization proved successful closure. |
| `workflow_result.terminal_state` | Stable terminal state from the shared vocabulary. |
| `workflow_result.terminal_reason` | Validated human-readable reason. |
| `workflow_result.required_next_action` | Validated action required after finalization, or why no further action is required. |
| `workflow_result.hollow_success_detected` | Whether the run looked successful but lacked completion evidence. |
| `workflow_result.reporting_failure` | Whether a reporting step failed after successful implementation (produces `FAILED_REPORTING` with durable evidence preserved). |
| `branch_diff_status` | `has_diff`, `no_diff`, or `unknown`. |

## Usage examples

### Re-run publish after a PR already exists

```bash
amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c branch_name=feat/issue-723-hygiene-cleanup \
  -c task_description="Add conservative hygiene cleanup"
```

If the branch already has an open PR, the recipe returns the existing PR URL and
does not call `gh pr create`.

### Re-run finalize after merge

```bash
amplihack recipe run workflow-finalize \
  -c repo_path=. \
  -c pr_number=723
```

If PR 723 is already merged, the recipe exits successfully with
`terminal_state=MERGED`.

### Treat a no-diff branch as done

```bash
amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c branch_name=feat/issue-723-noop
```

When `branch_name` has no diff against the detected base, publish exits
successfully and does not create an empty PR.

## Configuration

No feature flag is required. Resilience checks are always active for
`workflow-publish` and `workflow-finalize`.

GitHub operations still require a working `gh` installation and authentication
for GitHub repositories. The recipes must not print tokens, credential helper
output, auth headers, or full environment dumps.

Large nested workflow runs should preserve the project memory preference:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

This setting affects child Node tooling only. It does not relax terminal-state
or finalizer validation.

## Operator prerequisites for CI repair

A run that repairs CI pushes to the pull request branch as often as it needs to,
and has no round limit. Before relying on it:

- Give the run's token `contents`, `pull-requests` and `actions` write access
  and no administration or secrets permission. As a further safeguard, do not
  add the `workflow` scope. The repair rule does not depend on it: `.github/**` is
  never edited, whether or not the pull request changes it, so a red check
  whose fix, after diagnosis, must be made under `.github/` ends the run
  `BLOCKED_CI` as an owner decision, naming the file and its owner (the code
  owner of `.github/`, or the repository administrator).
- Protect the base branch: require a review, dismiss stale approvals when new
  commits are pushed, and forbid force-push and deletion. The repair rules
  forbid force-push and merging without an approval on the current head, but
  the branch protection is what enforces it.
- Cover `.github/` with `CODEOWNERS`. The repair never edits `.github/**`,
  but a rule in a prompt is advice and the code-owner review is the control.
- Do not configure deploy workflows to run on pushes to non-default branches.
  Each repair push would start one.

See [Red CI at Finalization](../reference/default-workflow-agentic-finalization.md#red-ci-at-finalization-2966)
for the safeguards and the limits of the design.

## Troubleshooting

| Symptom | Meaning | Action |
| --- | --- | --- |
| `closed-unmerged-with-diff` | A prior PR for this branch was closed without merge and the branch still has changes. | Reopen the PR, create a new branch intentionally, or re-run with an explicit branch after review. |
| `FAILED_CLOSED_UNMERGED` | Agentic finalization found a closed PR without merge evidence and local meaningful diff remains. | Reopen, supersede with a follow-up branch, or remove/merge the diff before rerunning. |
| `FAILED_MEANINGFUL_DIFF` | Local branch diff remains without accepted terminal publication, follow-up, no-op, or implementation-plus-verification evidence. | Publish the diff, create a durable follow-up, complete verification, or remove the unintended changes. |
| `branch_diff_status=unknown` | Git could not determine a safe base/head diff. | Check remotes, fetch state, and branch name. |
| `MANUAL_REQUIRED` | The repository needs a provider action that this workflow does not automate, such as creating an Azure Repos pull request. | Perform the named manual action, then rerun status/finalization with the durable change-request URL. |
| `BLOCKED_MANUAL_PROVIDER` | Required provider tooling, auth, permissions, or metadata is unavailable. | Fix the named blocker, then rerun the provider helper. |
| `BLOCKED_CI` | A check on the PR head fails, or cannot run, for a cause the run cannot fix itself. The stderr line and `terminal_reason` name the check and the cause. | Take the step named in `required_next_action` (for example, add the missing secret or approve the workflow run), then rerun. Pending or fixable failures do not end a run this way. |
| `FAILED_IMPLEMENTATION` | Durable implementation or verification evidence is absent or failed while meaningful work remains. An unfinished CI repair is not this state: the assessment is started again (#2966). | Resume `default-workflow` from implementation and verification, or choose a more specific failure state. |
| `FAILED_REPORTING` | Implementation succeeded but a reporting/finalization step failed. Durable `pr_url`/`pr_number` and implementation/verification evidence are preserved. | Re-run the reporting step; the implementation is intact. Do not re-run the implementation. |
| `HOLLOW_SUCCESS` | The run looked successful but its agent output was empty or inaccessible, so it may have produced nothing at all. Unchanged by #2343, and still ahead of every success classification. | Resume `default-workflow` from the missing phase or emit a supported no-op state with evidence. |
