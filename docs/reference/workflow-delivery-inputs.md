# Workflow Delivery Inputs Reference

> [Home](../index.md) > Reference > Workflow Delivery Inputs

`default-workflow` takes two optional sets of inputs that decide where a run
delivers its work. `base_branch` names the branch a pull request targets.
`strict_delivery` is for a caller that has already assigned the issue and the
branch, for example a dispatcher that hands queued work to many runs. The two
are independent: setting a base does not change how the issue is chosen.

## Contents

- [Inputs](#inputs)
- [`base_branch`](#base_branch)
- [`forbidden_base_branches`](#forbidden_base_branches)
- [`strict_delivery`](#strict_delivery)
- [How the issue is referenced](#how-the-issue-is-referenced)
- [What strict delivery does not do](#what-strict-delivery-does-not-do)
- [Example](#example)
- [Helpers](#helpers)
- [Tests](#tests)
- [Related documentation](#related-documentation)

## Inputs

| Key | Default | Meaning |
| --- | ------- | ------- |
| `base_branch` | `""` | Branch a new task branch starts from and the pull request targets. Empty keeps the remote default branch. |
| `forbidden_base_branches` | `""` | Whitespace-separated shell globs that `base_branch` must not match. |
| `strict_delivery` | `"false"` | Exactly `"true"` turns on the strict contract below. Any other value leaves it off. |
| `repository` | `""` | Strict delivery: the `owner/repo` on github.com that owns `issue_number`. |
| `pr_repository` | `""` | Strict delivery: the `owner/repo` that receives the pull request. Must be this checkout's `origin`. |
| `closing_reference` | `""` | Strict delivery: how the pull request names the issue, `#N` or `owner/repo#N` (see [below](#how-the-issue-is-referenced)). |

`issue_number`, `create_issue` and `existing_branch` keep their existing
meaning. Strict delivery requires them as described below. `repository`,
`pr_repository` and `closing_reference` are read only when `strict_delivery`
is `"true"`.

## `base_branch`

When `base_branch` is empty, nothing changes. Step-04 starts a new branch from
the remote default branch (`origin/HEAD`, then `origin/master`, then
`origin/develop`), step-16 calls `gh pr create` without `--base`, so GitHub
uses the repository's default branch, and finalize checks the pull request
against `origin/HEAD` as before.

When `base_branch` is set:

| Step | Behaviour |
| ---- | --------- |
| step-04, new branch | The branch starts from `origin/<base_branch>`. The receipt's `base_ref` and `base_branch` record it. |
| step-04, existing branch | `delivery_base_ref` checks the base before the worktree is set up, so a bad base fails before any agent runs. The receipt records `origin/<base_branch>`. |
| step-08c | The work verifier counts commits from `worktree_setup.base_ref`, or `origin/HEAD` when the receipt has no base. |
| step-14 | The version-bump agent compares against `worktree_setup.base_ref`, or `main` when the receipt has no base. |
| step-15, step-16 | The protected-branch guard refuses `main`, `master` and `base_branch`. A set `base_branch` adds a refused branch; it never removes one. |
| step-16 | The base is `origin/<base_branch>`, with no discovery or fallback. `gh pr create` gets `--base=<base_branch>`. On Azure DevOps the same base is the `--target-branch`. |
| terminal-state probe (publish and finalize) | The base is `origin/<base_branch>`. The diff, the commit count and the pull request's `baseRefName` are checked against it. |
| step-21 | `workflow_pr_ready.sh --base <base_branch>`: the scoped match and the `baseRefName` check before `gh pr ready` expect `base_branch`. |
| terminal adjudication, step-22b | `workflow_terminal_evidence.sh collect-for-step --base <base_branch>` and `workflow_final_status.sh --base <base_branch>`: the no-diff check, the scoped match and the "work already in the base" evidence use `base_branch`. |
| collect-finalization-evidence | `workflow_agentic_finalization.sh collect --base <base_branch>`: the evidence's `base_ref`, diff and commit count are taken from `origin/<base_branch>`. |

Finalize applies the rule step-16 applies: `base_branch` when it is set,
otherwise the discovery it always had. It reads the run's `base_branch`, not
the receipt's `base_ref`. For an ordinary new branch the receipt holds step-04's
own discovery, which need not be the branch GitHub chose for a pull request
opened without `--base`. When `base_branch` is set the two are the same branch.
`base_branch` reaches every finalize recipe the way `worktree_setup` does: each
recipe on the way declares the key, and the runner passes a parent's value only
into declared keys.

`delivery_base_ref` checks the value before it is used. The step fails, and no
other base is substituted, when `base_branch`:

- is not a valid branch name (`git check-ref-format --branch`);
- is written as a ref (`refs/...`) or as `@`;
- equals the branch being delivered;
- matches a pattern in `forbidden_base_branches`;
- has no remote-tracking ref `origin/<base_branch>`. Step-01 fetches every
  remote branch; the check itself never fetches.

GitHub applies closing keywords only when a pull request merges into the
repository's default branch. With any other base, `Closes` links the pull
request to the issue but does not close the issue when it merges.

## `forbidden_base_branches`

A caller that keeps branches nobody should target, for example coordination or
release branches, lists them here: `forbidden_base_branches: "release/* production"`.
Each word is a shell glob matched against the whole branch name. The bundle has
no built-in list. Separately, the step-15 and step-16 guard always refuses to
commit on, or open a pull request from, `main`, `master` or `base_branch`.

## `strict_delivery`

A strict run works on the issue and the branch its caller assigned. If any of
them cannot be confirmed, the run fails. It never creates, substitutes or
retargets anything.

Required inputs:

- `issue_number`: the issue's number;
- `repository`: the `owner/repo` that owns the issue, which may differ from
  this checkout's;
- `pr_repository`: this checkout's `origin` repository;
- `closing_reference`: the reference described
  [below](#how-the-issue-is-referenced);
- `existing_branch`: the branch to deliver. A strict run never derives one.

`create_issue` must not be set. `base_branch` is optional.

| Step | What strict delivery changes |
| ---- | ---------------------------- |
| step-03 | `select_strict_delivery_issue` checks the inputs above. `pr_repository` must be the checkout's `origin` and `closing_reference` must match. A set `base_branch` must pass `delivery_base_ref`. It then reads `repos/<repository>/issues/<N>` with `gh api --hostname github.com`. The read has a 30-second timeout and closed stdin. The answer must be an open issue, not a pull request, at `https://github.com/<repository>/issues/<N>`. Any failure ends the step with exit 2. The step does not read the task text, consult `remote_host_type`, create an issue or fall back to local tracking. |
| step-03c | Skipped: `existing_branch` is set (the rule from issue #1448). |
| step-04 | Uses `existing_branch`. Step-03 has already refused a strict run without one, and the new-branch path refuses a strict run as well. |
| step-06d | Reads the issue with `gh issue view N --repo github.com/<repository>`. A closed issue in this checkout's repository that shares the number no longer skips implementation. |
| step-08c | The work verifier reads the issue through the URL step-03 recorded, which names the owning repository. |
| step-15 | The commit message names the issue with `strict_issue_tag`, the rule step-03 and step-16 use: `owner/repo#N` when the issue is in another repository, compared case-insensitively. |
| step-16 | `pr_repository` and `closing_reference` are checked again against the checkout's `origin` before any write. The title suffix, summary and issue line use the reference. The issue title is read from `repository`. The issue claim check is skipped when the issue is in another repository (see below). |

The issue must be open, as on the ordinary path (#1595). A task that was
closed while it waited in the caller's queue, as completed or as not planned,
is not implemented: step-03 fails instead of opening a pull request that
closes an issue nobody wants done. A caller that wants a closed issue worked
on reopens it first.

The closing keyword is still chosen from the diff (#1361): `Closes` when the
change earns it, `Refs` otherwise. Strict delivery changes how the issue is
named, not whether the pull request may close it.

The issue claim check (#1361) asks whether a pull request in this repository
claims issue N of this repository. For an issue in another repository that is
the wrong question, so step-16 skips it and says so on stderr. The failure the
check exists for is a second run deriving a new branch for the same issue. A
strict run cannot do that, because it must publish the `existing_branch` its
caller assigned, and the scoped lookup has already found no pull request for
that head.

The issue is always read on github.com. Publication only accepts a github.com
`origin` (`parse_github_repo_identity`), and a closing reference cannot name an
issue on another host.

## How the issue is referenced

GitHub resolves a bare `#N` in the repository of the text that contains it.
The reference is therefore:

| Issue in | Pull request in | Reference |
| -------- | --------------- | --------- |
| `owner/repo` | `owner/repo` | `#N` |
| `plan/tasks` | `owner/repo` | `plan/tasks#N` |

Repository names compare case-insensitively, as on GitHub. `closing_reference`
must equal this reference exactly. Step-03 and step-16 fail when it does not.

## What strict delivery does not do

- It adds no text to any agent prompt. The guarantees come from the steps
  above, not from instructions to an agent. A caller that wants the agents told
  about its constraints puts them in `requirements`, which step-02 receives
  verbatim.
- It does not merge. Use `should_merge` and `no_merge` as before.
- It does not verify that GitHub linked the pull request to the issue. A
  caller that needs that confirms it itself.

## Example

```bash
amplihack recipe run default-workflow \
  -c task_description="Implement the queued task" \
  -c strict_delivery=true \
  -c issue_number=42 \
  -c repository=plan/tasks \
  -c pr_repository=owner/repo \
  -c closing_reference=plan/tasks#42 \
  -c existing_branch=feat/plan-tasks-42 \
  -c base_branch=dev \
  -c forbidden_base_branches='release/*'
```

## Helpers

| Helper | Used by | Purpose |
| ------ | ------- | ------- |
| `delivery_base_ref BASE BRANCH FORBIDDEN` (`tools/workflow_delivery_inputs.sh`) | step-03, step-04 (new and existing branch), step-16 | Checks `base_branch` and prints `origin/<base_branch>`. |
| `strict_issue_tag ISSUE REPOSITORY PR_REPOSITORY` | step-03, step-15, step-16 | Prints `#N` or `owner/repo#N`. |
| `same_github_repository A B` | step-03, step-16 | Case-insensitive `owner/repo` comparison. |
| `select_strict_delivery_issue` (`tools/workflow_issue_tracking.sh`) | step-03 | Confirms the caller's issue or fails. |

None of them read `RECIPE_VAR_*`. Each recipe passes the values it has
declared in its own `context` (#1561).

The finalize helpers take the base the same way, as an argument. An empty or
absent `--base` keeps their discovery; a set one is never replaced:

| Helper | Called by |
| ------ | --------- |
| `workflow_pr_ready.sh [--base BRANCH]` | step-21 |
| `workflow_final_status.sh [--base BRANCH]` | step-22b, and the terminal-state recipe's strict gate |
| `workflow_terminal_evidence.sh collect-for-step [--base BRANCH]` | the terminal adjudication recipe |
| `workflow_agentic_finalization.sh collect [--base BRANCH]` | collect-finalization-evidence |

## Tests

```bash
cargo test -p amplihack --test integration issue_1881_local_default_tracking::strict_delivery
cargo test -p amplihack --test integration workflow_publish_pr_metadata::recovery
cargo test -p amplihack --test integration workflow_delivery_inputs
```

## Related documentation

- [Tracking Issue Selection Reference](workflow-tracking-issue-selection.md) - how step-03 chooses the issue when `strict_delivery` is off
- [Issue Deduplication Reference](issue-dedup.md) - the issue claim check
- [Workflow GitHub Repo Identity](workflow-github-repo-identity.md) - the `origin` parser publication uses
