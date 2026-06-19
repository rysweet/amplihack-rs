# Workflow Publish and Finalize Resilience

> Compatibility note: this page now uses the provider-neutral workflow contract.
> GitHub PR publication can be automated. Azure DevOps, local, and unsupported
> change-request publication return explicit manual or blocked provider states
> instead of success-shaped `non-github` output.

`workflow-publish` and `workflow-finalize` classify already-terminal states
before taking PR actions. Re-running a workflow after a merge, after an
already-created PR, or on a branch with no diff produces a successful terminal
outcome instead of creating duplicate PRs or failing on already-finished work.

Finalization is agentic only where judgment is useful. Deterministic steps still
collect Git/PR/CI evidence, validate the finalizer schema, persist normalized
terminal fields, and return the process exit code. The agentic finalizer
classifies and explains the terminal state from structured evidence; malformed or
missing finalizer output fails closed.

## Observed failure modes

The resilience model exists because recent workflow runs and regression tests
exposed these concrete failure modes:

| Failure mode | Typical symptom | Resilient behavior |
| --- | --- | --- |
| Brittle parsing | A shell step inferred success from a PR URL, status string, or partial command output. | Only structured JSON/key-value evidence can prove terminal success. |
| Missing or stale PR metadata | `pr_number` or `pr_url` points to the wrong branch, stale head SHA, closed PR, or unavailable provider metadata. | PR identity must match repo, branch, base, and head SHA before PR state is trusted. |
| Dirty worktree misclassification | Generated artifacts or unstaged edits are treated as harmless no-diff work. | Dirty worktree blocks success with `FAILED_DIRTY_WORKTREE`. |
| Closed-unmerged PR handling | A closed PR without merge evidence is treated as completed while branch diff remains. | Finalization returns `FAILED_CLOSED_UNMERGED` unless local obsolete/no-diff proof supports `CLOSED_OBSOLETE`. |
| Remaining meaningful diff | Branch changes remain but no valid PR, merge, follow-up, or verified implementation path proves closure. | Finalization returns `FAILED_MEANINGFUL_DIFF` rather than treating the branch as a no-op success. |
| Missing tooling | Required `git`, `jq`, provider CLI, or provider auth is unavailable on a path that depends on it. | Finalization returns `FAILED_MISSING_TOOLING`, `FAILED_PR_METADATA_UNAVAILABLE`, or `BLOCKED_MANUAL_PROVIDER`; it does not silently skip required proof. |
| Failed CI | Open PR has failing required checks but final output looks complete. | Finalization returns `BLOCKED_CI` with failing check evidence and nonzero exit. |
| Hollow success | Workflow exits after setup, design, empty agent output, or inaccessible-codebase messages. | Finalization returns `HOLLOW_SUCCESS` or `FAILED_MISSING_TERMINAL_EVIDENCE`. |

## Publish behavior

Before creating a pull request, `workflow-publish` must classify the repository and
branch state.

| State | Publish result |
| --- | --- |
| Azure DevOps host | `ManualRequired` with an Azure Repos pull-request action; no `gh pr create` or `az repos pr create` call. |
| Local or unsupported host | `ManualRequired` with provider-neutral next action; no remote provider command. |
| No branch diff against base | Success with `state=no-diff`; no PR is created. |
| Existing open PR for the same head branch | Success with `state=existing-open-pr`; existing PR URL is returned. |
| Existing merged PR for the same head branch | Success with `state=already-merged`; merged PR URL is returned. |
| Existing closed PR that was merged | Success with `state=closed-after-merge`; merged PR URL is returned. |
| Existing closed, unmerged PR with branch diff | Failure with a clear action message. |
| Branch has diff and no existing PR | Create one draft PR and return its URL. |

The recipe must check branch diff and PR state before publishing. It never creates an
empty PR and never creates a second PR for the same live branch when an open or
merged PR already exists.

Closed-unmerged PRs with remaining branch diff are a hard failure. The workflow
must not silently republish, mark them complete, or hide the fact that user
action is required.

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
| Open PR with green required checks | Merge according to repository policy. |
| Open PR with pending or failed required checks | Failure with `terminal_state=BLOCKED_CI`; CI gating remains active. |
| Closed unmerged PR with remaining branch diff | Failure with `terminal_state=FAILED_CLOSED_UNMERGED`; user action is required. |
| Meaningful branch diff without terminal publication or verification proof | Failure with `terminal_state=FAILED_MEANINGFUL_DIFF`. |
| Missing or ambiguous PR state | Failure; ambiguity is not treated as success. |
| Missing or malformed finalizer output | Failure with `terminal_state=FAILED_FINALIZER_OUTPUT`. |
| Success-looking run without completion evidence | Failure with `terminal_state=HOLLOW_SUCCESS` or `FAILED_MISSING_TERMINAL_EVIDENCE`. |

Already-terminal states are success only when verified through Git branch diff,
GitHub PR metadata, or both. Pending or failed required checks on active PRs
remain failures; terminal-state resilience must not bypass CI.

## Recipe context reference

These context keys influence publish/finalize behavior:

| Context key | Used by | Meaning |
| --- | --- | --- |
| `repo_path` | publish, finalize | Repository root to inspect. Defaults to the recipe working directory. |
| `provider_context` | publish, finalize | Structured provider helper result. `GitHub` enables GitHub PR handling; `AzureDevOps`, `Local`, and `Unsupported` use manual or blocked publication states. |
| `remote_host_type` | publish | Legacy compatibility input. Implementations may accept `github`, `azure-devops`, `azdo`, or `local`, but must normalize to `provider_context` before provider operations. |
| `branch_name` | publish, finalize | Current workflow branch or explicit branch to inspect. |
| `base_branch` | publish, finalize | Base branch for diff checks. Defaults to the detected remote default branch. |
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
| `workflow_result.finalizer_output_valid` | Whether structured finalizer output passed schema validation. |
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

## Troubleshooting

| Symptom | Meaning | Action |
| --- | --- | --- |
| `closed-unmerged-with-diff` | A prior PR for this branch was closed without merge and the branch still has changes. | Reopen the PR, create a new branch intentionally, or re-run with an explicit branch after review. |
| `FAILED_CLOSED_UNMERGED` | Agentic finalization found a closed PR without merge evidence and local meaningful diff remains. | Reopen, supersede with a follow-up branch, or remove/merge the diff before rerunning. |
| `FAILED_MEANINGFUL_DIFF` | Local branch diff remains without accepted terminal publication, follow-up, no-op, or implementation-plus-verification evidence. | Publish the diff, create a durable follow-up, complete verification, or remove the unintended changes. |
| `branch_diff_status=unknown` | Git could not determine a safe base/head diff. | Check remotes, fetch state, and branch name. |
| `MANUAL_REQUIRED` | The repository needs a provider action that this workflow does not automate, such as creating an Azure Repos pull request. | Perform the named manual action, then rerun status/finalization with the durable change-request URL. |
| `BLOCKED_MANUAL_PROVIDER` | Required provider tooling, auth, permissions, or metadata is unavailable. | Fix the named blocker, then rerun the provider helper. |
| `BLOCKED_CI` | Required checks are pending, failed, or unavailable when required. | Fix CI or wait for checks; terminal-state resilience does not bypass CI. |
| `FAILED_FINALIZER_OUTPUT` | The agentic finalizer returned malformed, missing, or schema-invalid JSON. | Inspect the finalizer step output and rerun after fixing the prompt/schema/tooling path. |
| `HOLLOW_SUCCESS` | The run looked successful but produced no implementation, verification, publish, or valid no-op evidence. | Resume `default-workflow` from the missing phase or emit a supported no-op state with evidence. |
