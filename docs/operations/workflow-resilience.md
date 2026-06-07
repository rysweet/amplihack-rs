# Workflow Publish and Finalize Resilience

`workflow-publish` and `workflow-finalize` classify already-terminal states
before taking PR actions. Re-running a workflow after a merge, after an
already-created PR, or on a branch with no diff produces a successful terminal
outcome instead of creating duplicate PRs or failing on already-finished work.

## Publish behavior

Before creating a pull request, `workflow-publish` must classify the repository and
branch state.

| State | Publish result |
| --- | --- |
| Non-GitHub host | Success with `state=non-github`; no `gh pr create` call. |
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

`workflow-finalize` must use the same terminal-state model.

| State | Finalize result |
| --- | --- |
| No diff and no PR required | Success with `terminal_status=no-diff`. |
| PR already merged | Success with `terminal_status=already-merged`. |
| PR closed after merge | Success with `terminal_status=closed-after-merge`. |
| Open PR with green required checks | Merge according to repository policy. |
| Open PR with pending or failed required checks | Failure; CI gating remains active. |
| Closed unmerged PR with remaining branch diff | Failure; user action is required. |
| Missing or ambiguous PR state | Failure; ambiguity is not treated as success. |

Already-terminal states are success only when verified through Git branch diff,
GitHub PR metadata, or both. Pending or failed required checks on active PRs
remain failures; terminal-state resilience must not bypass CI.

## Recipe context reference

These context keys influence publish/finalize behavior:

| Context key | Used by | Meaning |
| --- | --- | --- |
| `repo_path` | publish, finalize | Repository root to inspect. Defaults to the recipe working directory. |
| `remote_host_type` | publish | Host classification. `github` enables GitHub PR handling; `azure-devops` and `local` skip GitHub publishing. `azure-devops` is the public value; implementations may accept `azdo` as a legacy alias but should normalize outputs to `azure-devops`. |
| `branch_name` | publish, finalize | Current workflow branch or explicit branch to inspect. |
| `base_branch` | publish, finalize | Base branch for diff checks. Defaults to the detected remote default branch. |
| `pr_number` | finalize | Existing PR to finalize when known. |
| `pr_url` | finalize | Existing PR URL to parse when `pr_number` is absent. |
| `issue_number` | publish | Issue/work item identifier used in PR title/body generation. |
| `task_description` | publish | Human-readable task text used in PR title/body generation. |

Recipe outputs include:

| Output key | Meaning |
| --- | --- |
| `pr_publish_result.pr_url` | Created or reused PR URL, when a GitHub PR exists. |
| `pr_publish_result.pr_number` | Created or reused PR number, when available. |
| `pr_publish_result.state` | Publish classifier result. |
| `workflow_result.terminal_outcome` | Final workflow terminal classifier result. |
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
`terminal_status=already-merged`.

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

## Troubleshooting

| Symptom | Meaning | Action |
| --- | --- | --- |
| `closed-unmerged-with-diff` | A prior PR for this branch was closed without merge and the branch still has changes. | Reopen the PR, create a new branch intentionally, or re-run with an explicit branch after review. |
| `branch_diff_status=unknown` | Git could not determine a safe base/head diff. | Check remotes, fetch state, and branch name. |
| `non_github_host` | The repository is Azure DevOps or local-only. | Use the provider-specific workflow path; GitHub PR creation is intentionally skipped. |
| Active PR fails finalize | Required checks are pending or failed. | Fix CI or wait for checks; terminal-state resilience does not bypass CI. |
