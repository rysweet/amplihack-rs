---
title: Workflow Delivery Inputs Reference
description: Base branch, strict delivery and configurable issue references for workflow publication.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Workflow Delivery Inputs Reference

> [Home](../index.md) > Reference > Workflow Delivery Inputs

`default-workflow` takes optional inputs that control delivery and issue
references. `base_branch` names the branch a pull request targets.
`strict_delivery` is for a caller that has already assigned the issue and the
branch, for example a dispatcher that hands queued work to many runs. The two
are independent: setting a base does not change how the issue is chosen.
The `pr_issue_link` input selects the generated issue keyword
while retaining the discovered issue. [Where the inputs apply](#where-the-inputs-apply) lists the
delivery paths; [issue keyword propagation](#issue-keyword-propagation) lists
the recipe boundaries for `pr_issue_link`.

## Contents

- [Inputs](#inputs)
- [`base_branch`](#base_branch)
- [`forbidden_base_branches`](#forbidden_base_branches)
- [`strict_delivery`](#strict_delivery)
- [How the issue is referenced](#how-the-issue-is-referenced)
- [`pr_issue_link`](#pr_issue_link)
- [Published references and description edits](#published-references-and-description-edits)
- [What strict delivery does not do](#what-strict-delivery-does-not-do)
- [How the inputs reach each phase](#how-the-inputs-reach-each-phase)
- [Where the inputs apply](#where-the-inputs-apply)
- [Example](#example)
- [Helpers](#helpers)
- [Tests](#tests)
- [Related documentation](#related-documentation)

## Inputs

| Key | Default | Meaning |
| --- | ------- | ------- |
| `base_branch` | `""` | Branch a new task branch starts from and the pull request targets. Empty keeps the remote default branch. |
| `forbidden_base_branches` | `""` | Shell globs that `base_branch` must not match, separated by spaces, tabs or newlines (see [below](#forbidden_base_branches)). |
| `strict_delivery` | `"false"` | Exactly `"true"` turns on the strict contract below. `"false"` or empty leaves it off. Step-03 fails on any other value, such as `1`, `yes` or `TRUE`. |
| `repository` | `""` | Strict delivery: the `owner/repo` on github.com that owns `issue_number`. |
| `pr_repository` | `""` | Strict delivery: the `owner/repo` that receives the pull request. Must be this checkout's `origin`. |
| `closing_reference` | `""` | Strict delivery: how the pull request names the issue, `#N` or `owner/repo#N` (see [below](#how-the-issue-is-referenced)). |
| `pr_issue_link` | `"auto"` | Generated GitHub issue keyword: exactly `auto`, `refs` or `closes`. Applies independently of strict delivery. |

`issue_number`, `create_issue` and `existing_branch` keep their existing
meaning. Strict delivery requires them as described below. `repository`,
`pr_repository` and `closing_reference` are read only when `strict_delivery`
is `"true"`. Step-03 fails a run that sets any of them while `strict_delivery`
is off, instead of running the ordinary path while the caller believes the
strict contract holds.

`create_issue` accepts `true`, `1` or `yes`; `strict_delivery` accepts only
`true`. The difference is deliberate. Strict delivery exists to fail closed, so
a spelling it does not recognise stops the run at step-03, where the caller
sees it, instead of turning the contract off.

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
`base_branch` reaches every finalize recipe because the runner passes a
parent's whole context into each sub-recipe; see
[How the inputs reach each phase](#how-the-inputs-reach-each-phase).

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

Words are separated by spaces, tabs or newlines, and every line is read. A
YAML block scalar with one pattern per line, or a list joined with `\n`,
works:

```yaml
forbidden_base_branches: |
  release/*
  production
```

The list is plain words, not JSON or YAML list syntax. An entry that contains
a double quote, a single quote or a comma, or that starts with `[`, makes
`delivery_base_ref` fail with the entry named. That catches
`["release/*","production"]`, `[release/*, production]` and
`release/*,production`. Read as globs, each of those would match almost
nothing, so the list would look enforced while it let every base through. A
partly understood deny list is refused, never applied in part.

Two things follow from that rule. A branch whose name contains a quote or a
comma is matched with `?` in that position (`team?a` matches `team,a`). A glob
cannot start with a bracket expression; write the alternatives as separate
words (`Release/* release/*` rather than `[Rr]elease/*`). Git branch names
cannot contain `[`, so a leading `[` was always glob syntax and never part of
a name.

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
| step-03 | `delivery_strict_mode` first refuses a `strict_delivery` other than `"true"`, `"false"` or empty, and `repository`, `pr_repository` or `closing_reference` set without strict delivery. Then `select_strict_delivery_issue` checks the inputs above. `pr_repository` must be the checkout's `origin` and `closing_reference` must match. A set `base_branch` must pass `delivery_base_ref`. It then reads `repos/<repository>/issues/<N>` with `gh api --hostname github.com`. The read has a 30-second timeout and closed stdin. The answer must be an open issue, not a pull request, at `https://github.com/<repository>/issues/<N>`. Any failure ends the step with exit 2. The step does not read the task text, consult `remote_host_type`, create an issue or fall back to local tracking. |
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

The keyword follows the caller mode and existing diff classification (#1361).
Strict delivery determines the issue's identity. The
[`pr_issue_link`](#pr_issue_link) policy determines whether an eligible
change uses a closing keyword.

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

`closing_reference` contains the reference alone, without `Refs` or `Closes`.
Its name does not require a closing keyword: `pr_issue_link=refs`
preserves the same reference with `Refs`.

## `pr_issue_link`

`-c pr_issue_link=refs` keeps generated references non-closing, for example
when several PRs contribute to one issue. The values are exactly `auto`,
`refs` and `closes`; omitting the input selects `auto`.

| Existing change classification | Omitted / `auto` | `refs` | `closes` |
| --- | --- | --- | --- |
| Substantive | `Closes #N` | `Refs #N` | `Closes #N` |
| Planning-only | `Refs #N` | `Refs #N` | `Refs #N` |
| Generated-only | `Refs #N` | `Refs #N` | `Refs #N` |
| Empty | `Refs #N` | `Refs #N` | `Refs #N` |
| Classification unavailable or failed | `Refs #N` | `Refs #N` | `Refs #N` |

The table assumes discovery found an issue. The mode changes only its
keyword. It preserves discovery, path classification and reference identity,
including `owner/repo#N` for another repository. When discovery finds no
issue, the workflow adds no issue reference. Azure DevOps retains its existing
work-item behaviour, including `AB#N` commit references.

Substantive means at least one changed path is substantive under the existing
classifier. Planning documents and generated files alone never earn a
closing keyword, even with `closes`. Ordinary documentation can be
substantive; a documentation-only change is not automatically planning-only.

### Commit and PR classification

Step-15 classifies staged paths for generated commit messages. Step-16
classifies the complete PR diff against its base for the description. Both
apply the same mode-aware policy to their respective classification results.
An empty set of changed paths produces `Refs` if an issue is known; it does not
cause an otherwise empty commit or PR to be created.

A missing or failing classifier, or a failed keyword selector, falls back to
`Refs` in every mode. A classifier that exits unsuccessfully cannot establish
eligibility even if it prints `Closes`. An explicit `closes` therefore never
turns an unclassified change into a closing reference.

The shared `amplihack workflow issue-keyword` selector returns
exactly `Refs` or `Closes`. Its mode defaults to `auto`; its optional
classification accepts the successful classifier verdict `Refs` or `Closes`. An omitted
classification means unavailable and returns `Refs`. Adapters retain the
classifier's exit status and accept only a successful selector result that
matches one of those two keywords.

If rich PR body construction fails after classification, the minimal
body reuses the selected keyword and reference. For example, a substantive
change with `auto` retains `Closes #N`; with `refs`, it retains `Refs #N`.

### Issue keyword propagation

Each recipe declares `pr_issue_link: "auto"` and explicitly forwards the
caller's value along this path:

| Boundary | Value passed |
| --- | --- |
| `auto-drive-to-merge` → `autodrive-build` | Caller-selected `pr_issue_link` |
| `autodrive-build` → `default-workflow` | Same value |
| `default-workflow` → `workflow-publish` | Same value |
| `workflow-publish` → `workflow_publish_pr.sh` | `RECIPE_VAR_pr_issue_link` |

The `default-workflow` bookkeeping includes the key. Direct
invocations use the same default. Publishing reads
`RECIPE_VAR_pr_issue_link`; an inherited bare `pr_issue_link` alias cannot
override it. A new invocation with the input omitted uses `auto` rather than
a stale inherited value.

### Keep the issue open after publication

This invocation builds work for issue #2550 with a non-closing reference:

```bash
amplihack recipe run auto-drive-to-merge --detach \
  -c task_description="Implement rysweet/amplihack-rs#2550" \
  -c pr_issue_link=refs \
  -c no_merge=true
```

With this policy, a substantive change's generated commit and PR
description contain `Refs #2550`. `no_merge=true` leaves the PR open for review;
`pr_issue_link=refs` preserves the non-closing reference if the PR is later merged. Omitting
`pr_issue_link`, or supplying `auto` or `closes`, produces `Closes #2550` for
that substantive change. Planning-only changes produce `Refs #2550` in every
mode.

## Published references and description edits

Under this contract, the published keyword and reference identity are
authoritative. Finalisation, merge-ready and later description edits preserve
them without reclassifying the diff or adding a closing reference. Running with a different mode against
an existing PR does not rewrite its published reference.

Merge-ready accepts a published `Refs #N` or `Refs owner/repo#N` as issue-link
evidence even when GitHub's `closingIssuesReferences` is empty. Separate
project requirements for linked issues or work items still apply.

Readiness editing fetches the current description and composes an update with
`amplihack workflow compose-readiness-description` before submitting it. The
command reads the current description and replacement report as
required JSON string fields `description` and `report` on standard input and buffers the composed body
before writing it to standard output. The composer preserves the description
verbatim before the established
`<!-- merge-ready-report` marker or legacy `## Merge readiness` heading and
replaces the report below that boundary. When no report exists, it appends
one below the description. Conflicting boundaries are rejected.

Use the executable [fetch-compose-edit example](../../amplifier-bundle/skills/merge-ready/SKILL.md#fetch-compose-edit),
with trusted `PR` and private `REPORT_FILE` inputs. It validates the fetched
body, JSON-encodes both fields using `jq`, composes through Rust, and submits
only successful output through `gh pr edit --body-file`. It then reads the
body back and compares it. Description and report text remain data.

New report content cannot introduce closing references: `close`, `closes`,
`closed`, `fix`, `fixes`, `fixed`, `resolve`, `resolves` and `resolved`, in any
case, with bare, cross-repository or issue-URL references are rejected,
including Markdown formatting and multiple references. Validation checks the
original report, link labels and resolved issue destinations. Inline links and
full, collapsed and shortcut reference links cannot hide a closing keyword.
Reference definitions in the preserved description also resolve new report
links; an earlier definition takes priority over a duplicate in the report.
Ordinary prose such as "fixes pagination" and issue evidence without a closing
keyword remain valid. The composer uses a CommonMark parser to resolve links
while retaining labels and reference definitions. Validation applies to the new
report; existing published `Closes` or `Refs` text is preserved.
Formatting removal does not expose an ordinary link's hidden destination as a
closing keyword.
Malformed input or validation failure produces no composed body. JSON diagnostics
include only the error category, line and column, never supplied values or report
excerpts. A failed fetch, parse or composition leaves the platform description
untouched; an edit is submitted only after successful composition and then read
back.

The squash body comes from the published description above the report
boundary, retaining `Refs #N` verbatim. Readiness material is removed, and
individual commit messages and their issue trailers are not concatenated.
No separate squash-message option is needed. See the
[squash commit message reference](auto-drive-to-merge.md#the-squash-commit-message-2056).

## What strict delivery does not do

- It adds no text to any agent prompt. The guarantees come from the steps
  above, not from instructions to an agent. A caller that wants the agents told
  about its constraints puts them in `requirements`, which step-02 receives
  verbatim.
- It does not merge. Use `should_merge` and `no_merge` as before.
- It does not verify that GitHub linked the pull request to the issue. A
  caller that needs that confirms it itself.

## How the inputs reach each phase

`default-workflow` runs its phases as sub-recipes in the same runner process.
The runner (`recipe-runner-rs`) builds each sub-recipe's context from the
sub-recipe's own `context:` defaults, then lays the parent's whole context
over them, then applies the calling step's `context:` overrides. When the
sub-recipe finishes, its context is merged back into the parent's. Every key
in the result is exported to bash steps as `RECIPE_VAR_<key>`, whether or not
the sub-recipe declares it.

A recipe declaration does not filter what a parent passes. Each recipe that
reads an existing delivery input still declares it, with an empty or `"false"`
default. The `pr_issue_link` declarations default to
`"auto"`. Declarations serve two purposes:

- the default applies when the recipe runs on its own, with no parent;
- a nested `amplihack recipe run` in a separate process inherits the parent
  process's environment, and the declaration replaces an inherited
  `RECIPE_VAR_<key>` with the run's own value, so a stale value is never read
  (#1561).

Because the whole context flows down, a parent recipe that keeps a key named
`base_branch`, `forbidden_base_branches`, `strict_delivery`, `repository`,
`pr_repository` or `closing_reference` for its own purposes passes that value
to `default-workflow` when it calls it in-process. What happens then:

| Key held by the parent | Effect on the run |
| ---------------------- | ----------------- |
| `repository`, `pr_repository`, `closing_reference` | Step-03 fails unless `strict_delivery` is `"true"`. |
| `strict_delivery` | Step-03 fails unless the value is `"true"`, `"false"` or empty. |
| `base_branch` | Taken as the base. It must still pass `delivery_base_ref`, and step-04's receipt records it, but nothing can tell it apart from a value meant for this run. |
| `forbidden_base_branches` | Taken as the deny list. A list in the wrong syntax fails the run when `base_branch` is set. |

No bundled recipe gives any of these keys a value of its own. `quick-change`
and `auto-drive-light` clear `base_branch` and `forbidden_base_branches`, and
`smart-classify-route` declares `strict_delivery` so that it can read it; see
[Where the inputs apply](#where-the-inputs-apply). A parent that needs one of
these names for something else clears it in the calling step's `context:`
(for example `base_branch: ""`) before it calls `default-workflow`, or uses
another name.

## Where the inputs apply

The inputs take effect where `default-workflow` runs: on its own, or in-process
under another recipe, such as `smart-orchestrator`'s development route when the
task stays on one workstream. Two recipes start a branch and open a pull
request without `default-workflow`'s publish phase, and one route runs
`default-workflow` in separate processes:

| Path | `base_branch`, `forbidden_base_branches` | `strict_delivery` |
| ---- | ---------------------------------------- | ----------------- |
| `quick-change`, and `auto-drive-light` for a new change | Cleared in the call to `workflow-worktree`. The `quick/` branch starts from `origin/HEAD`, and the pull request targets the branch `origin/HEAD` names, so the two agree. A `base_branch` from the caller is not used. | Left as inherited. Step-04 refuses to create a branch when it is `"true"`, so a strict run stops before any agent runs. |
| `smart-orchestrator`, task split into parallel workstreams | Not forwarded. `launch-parallel-round-1` passes only `create_issue` to `amplihack orch run`. Each development workstream runs `default-workflow` in its own process, whose declared defaults replace any inherited value, so each branch starts from, and each pull request targets, the remote default branch. | Never split. `activate-workflow` keeps the run on one workstream when `strict_delivery` is anything but empty or `"false"`, and says so on stderr. |

The runner merges a sub-recipe's context back into its caller's. Once
`quick-change` or `auto-drive-light` has called `workflow-worktree`,
`base_branch` and `forbidden_base_branches` are empty for the rest of the run,
in that recipe and in the recipes that called it. A `quick-change` hand-over
to `default-workflow` therefore continues the `quick/` branch with no
`base_branch`, and its pull request also targets the remote default branch.

`auto-drive-light` continuing an existing branch or pull request
(`existing_branch` or `pr_number` set) does not call `workflow-worktree` and
opens no pull request.

A strict development task that `smart-orchestrator` keeps on one workstream
goes to `default-workflow` in-process, which applies the contract above. `quick-change`
is not chosen, because strict delivery requires `existing_branch`, which rules
it out (#1740); a strict run without `existing_branch` is refused by step-03 of
`default-workflow` or by step-04 under `quick-change`.

To deliver to `base_branch` through `smart-orchestrator`, pass
`force_single_workstream=true` and `workflow=default-workflow`, which keep the
task on one workstream and off `quick-change`, or call `default-workflow`
directly.

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
| `delivery_base_ref BASE BRANCH FORBIDDEN` (`tools/workflow_delivery_inputs.sh`) | step-03, step-04 (new and existing branch), step-16 | Checks `base_branch` and `forbidden_base_branches` and prints `origin/<base_branch>`. |
| `delivery_strict_mode STRICT REPOSITORY PR_REPOSITORY CLOSING_REFERENCE` | step-03 | Prints `true` or `false`, or fails on an unrecognised `strict_delivery` or a strict-only input without strict delivery. |
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

The issue-link integration tests require the repository-pinned recipe runner
installed by `scripts/install-pinned-recipe-runner.sh`. They use private homes,
local repositories, recording hooks and fake hosting commands; no credentials
or live hosting service are needed. The runner tests retain the production
context declarations and nested mappings through all four recipes.

```bash
cargo test -p amplihack-cli --lib commands::workflow::
cargo test -p amplihack --test integration workflow_issue_link::
cargo test -p amplihack --test integration workflow_readiness_description::
cargo test -p amplihack --test integration issue_1881_local_default_tracking::strict_delivery
cargo test -p amplihack --test integration workflow_publish_pr_metadata::recovery
cargo test -p amplihack --test integration workflow_delivery_inputs
```

## Related documentation

- [Tracking Issue Selection Reference](workflow-tracking-issue-selection.md) - how step-03 chooses the issue when `strict_delivery` is off
- [Issue Deduplication Reference](issue-dedup.md) - the issue claim check
- [Workflow GitHub Repo Identity](workflow-github-repo-identity.md) - the `origin` parser publication uses
- [Auto Drive To Merge Reference](auto-drive-to-merge.md) - caller configuration, finalisation and squash messages
