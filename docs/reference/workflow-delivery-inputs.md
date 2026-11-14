# Workflow Delivery Inputs Reference

> [Home](../index.md) > Reference > Workflow Delivery Inputs

`default-workflow` takes two optional sets of inputs that decide where a run
delivers its work. `base_branch` names the branch a pull request targets.
`strict_delivery` is for a caller that has already assigned the issue and the
branch, for example a dispatcher that hands queued work to many runs. The two
are independent: setting a base does not change how the issue is chosen.
They apply where `default-workflow` runs; [Where the inputs apply](#where-the-inputs-apply)
lists the paths that do not use them.

## Contents

- [Inputs](#inputs)
- [`base_branch`](#base_branch)
- [How finalize finds the base](#how-finalize-finds-the-base)
- [`forbidden_base_branches`](#forbidden_base_branches)
- [`strict_delivery`](#strict_delivery)
- [How the issue is referenced](#how-the-issue-is-referenced)
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
| `base_branch` | `""` | Branch a new task branch starts from and the pull request targets. Empty keeps the remote default branch for the new branch and the new pull request; finalize then takes the base from the pull request the run opened. |
| `forbidden_base_branches` | `""` | Shell globs that `base_branch` must not match, separated by spaces, tabs or newlines (see [below](#forbidden_base_branches)). |
| `strict_delivery` | `"false"` | Exactly `"true"` turns on the strict contract below. `"false"` or empty leaves it off. Step-03 fails on any other value, such as `1`, `yes` or `TRUE`. |
| `repository` | `""` | Strict delivery: the `owner/repo` on github.com that owns `issue_number`. |
| `pr_repository` | `""` | Strict delivery: the `owner/repo` that receives the pull request. Must be this checkout's `origin`. |
| `closing_reference` | `""` | Strict delivery: how the pull request names the issue, `#N` or `owner/repo#N` (see [below](#how-the-issue-is-referenced)). |

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

When `base_branch` is empty, step-04 starts a new branch from the remote
default branch: the first of `origin/HEAD`, `origin/master` and
`origin/develop` that exists. Step-04's list has no `origin/main`; the finalize
helpers use a different list, given in rule 5 of
[How finalize finds the base](#how-finalize-finds-the-base). Step-16 calls `gh pr create` without `--base`, so GitHub uses the
repository's default branch. Finalize does not assume either of those. It
checks the pull request against the base that pull request actually has, as
described in [How finalize finds the base](#how-finalize-finds-the-base). A
local `origin/HEAD` that is out of date, for example after the repository's
default branch on GitHub changed to `dev`, no longer makes step-21 miss the
pull request.

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

Finalize reads the run's `base_branch`, not the receipt's `base_ref`. For an
ordinary new branch the receipt holds step-04's own discovery, which need not
be the branch GitHub chose for a pull request opened without `--base`. When
`base_branch` is set the two are the same branch. `base_branch` reaches every
finalize recipe because the runner passes a parent's whole context into each
in-process sub-recipe. Every bundled recipe that calls `default-workflow` also
declares the key and passes it on explicitly, so the value still arrives when a
call runs in a separate process; see
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

## How finalize finds the base

Step-21 (`workflow_pr_ready.sh`), step-22b (`workflow_final_status.sh`), the
terminal evidence (`workflow_terminal_evidence.sh`), the finalization evidence
(`workflow_agentic_finalization.sh collect`) and the terminal-state probe all
use one function, `delivery_run_base`, to decide which base the run's pull
request has. It takes the first rule that gives a value:

| Order | Source | Checks |
| ----- | ------ | ------ |
| 1 | The run's `base_branch`, when it is set. | Format, `refs/*` and `@` checks. A pull request with another base still fails closed. |
| 2 | `base_branch` in step-16's publish result (`pr_publish_result.base_branch`). | Rule 4. |
| 3 | The `baseRefName` of the pull request this run recorded, found by `pr_url` or `pr_number` with one read-only `gh pr view`. | The reference and rule 4. |
| 4 | Not a source: the value from rule 2 or 3 goes through `delivery_base_ref`. | See below. |
| 5 | Only when the run recorded no pull request at all: the old discovery, `delivery_default_base`, which gives the first of `origin/HEAD`, `origin/main`, `origin/master` and `origin/develop` that exists. This is the finalize helpers' own list, not step-04's. | As before. |

An explicit `base_branch` always wins. It is never replaced by the pull
request's base, so `base_branch=main` with a pull request into `dev` still
ends in `no_scoped_pr`, and `gh pr ready` is not called.

A base taken from the publish result or from the pull request is data, not
input from the caller, so it is checked before it is used. `delivery_base_ref`
refuses it, and the helper fails with `unable to resolve expected base branch`,
when the name:

- is not a valid branch name, is written as a ref (`refs/...`) or as `@`;
- has no remote-tracking ref `refs/remotes/origin/<base>`. A local branch or
  tag named `origin/<base>` does not count;
- matches a pattern in `forbidden_base_branches`;
- equals the branch being delivered.

Rule 3 runs `gh pr view` only for a reference that is a positive number or a
`https://github.com/<owner>/<repo>/pull/<N>` URL whose repository is this
checkout's `origin`. Anything else fails before `gh` runs. The output is read
with `jq` for `baseRefName` and nothing else.

Once the run has recorded a pull request, `origin/HEAD` and the
`origin/main`/`origin/master`/`origin/develop` list are never used, even when `gh pr view` fails. The
helper fails closed instead, so a stale `origin/HEAD` cannot send finalize to
the wrong pull request, or tell it there is none. Step-22b, which only writes
the final report, is the one exception; see
[When step-22b cannot read the base](#when-step-22b-cannot-read-the-base).

Each helper works the base out once and uses that one value for both the
scoped lookup (`workflow_pr_scope.sh --base`) and the identity check before
`gh pr ready`. `workflow_pr_scope.sh` does not change: it still requires
`--base` and still compares the head branch, head commit, head repository and
`baseRefName`. A pull request this run did not open is never found by a
search.

Step-16 writes `base_branch` into the publish result only when the base was
checked: the base it passed to `gh pr create --base`, or the `baseRefName` of
an existing pull request whose identity it confirmed. A pull request created
without `--base`, the no-diff exit and every error exit record `""`. A run
whose publish result has no `base_branch`, including runs from before the
field existed, uses rule 3. A recorded base is still compared with the live
pull request's `baseRefName` by the scope and identity checks.

The two evidence steps (`workflow_terminal_evidence.sh` and
`workflow_agentic_finalization.sh collect`) only collect evidence. When
`delivery_run_base` fails they fall back to the old discovery and write one
line to stderr naming the base they used. The terminal-state probe never
reports success when the base cannot be worked out: a pull request that does
not match ends in `FAILED_INVALID_INPUT`, as before.

### When step-22b cannot read the base

Step-22b (`workflow_final_status.sh`) prints the run's final status. It
changes nothing on GitHub, so it does not fail just because the base cannot be
read. Step-21 (`gh pr ready`) and the terminal-state probe still fail closed,
and they never use the fallback below.

`delivery_run_base` returns 2 when the base is *unreadable* and 1 when it is
*refused*, so step-22b tells the two apart by exit code, without reading error
text. It returns 2 only from rule 3, when every one of these holds:

- `--base` is empty;
- the publish result records no `base_branch`;
- the recorded pull request reference is a
  positive number, or a `https://github.com/<owner>/<repo>/pull/<N>` URL for
  this checkout's `origin`;
- `gh pr view` then failed, returned something other than JSON, or returned no
  `baseRefName`.

On exit code 2 only, step-22b takes the repository's default branch from
`delivery_default_base`. That is the first of `origin/HEAD`'s target,
`origin/main`, `origin/master` and `origin/develop` that exists locally. The
candidate still goes through `delivery_base_ref` with
`forbidden_base_branches`. Step-22b prints one warning on stderr, with the
branch name quoted by `printf '%q'`:

```text
WARNING: workflow_final_status.sh: the pull request's base could not be read; reporting against the default branch main
```

The fallback base is used for one thing only: the scoped match
(`workflow_pr_scope.sh --base`). The scope helper still compares `--base` with
the live pull request's `baseRefName`. A wrong guess therefore gives a miss,
never a false match, and the #1268 adjudication then decides the terminal
state. The no-diff check for a `no-diff`, `NO_DIFF_SUCCESS` or
`CLOSED_OBSOLETE` publish state needs a base read from the run or the pull
request itself: with a fallback base it fails with "could not confirm that
state", as it did before #2520, and never promotes `no-diff` to
`NO_DIFF_SUCCESS`.

A fallback base never creates a success state. The terminal state, for example
`EXISTING_OPEN_PR`, comes from the publish evidence before the scoped match
runs. The match can only confirm that state or veto it: a miss sends the run
to the #1268 adjudication, and a match leaves the state as it was.

The terminal-state recipe's strict gate (`strict-terminal-state` in
`workflow-terminal-state.yaml`) runs the same `workflow_final_status.sh` with
`--base "${RECIPE_VAR_base_branch:-}"`. With an empty `base_branch` it gets the
same fallback, and it passes when the fallback match confirms the success
state the publish evidence set. That is safe: `workflow_pr_scope.sh` keeps
only pull requests whose live `baseRefName` equals `--base`, so a match means
the pull request's base really is the default branch.

A base that is *refused* (exit code 1) is never replaced. The fallback does
not run when:

- an explicit or recorded `base_branch` exists, whatever `gh` returns;
- the pull request reference is malformed or names another repository;
- `gh pr view` returned a `baseRefName` that `delivery_base_ref` refuses;
- the default branch itself is refused by `delivery_base_ref`.

`delivery_base_ref` refuses a name that is not a valid branch name, is written
as a ref (`refs/...`) or as `@`, has no `refs/remotes/origin/<base>`, matches
`forbidden_base_branches`, or equals the branch being delivered.

When no base can be found at all, step-22b skips the scoped match and does not
claim one. It prints no `lacks … context` error for the base. That error still
appears when the repository, branch or head commit is missing. The terminal
state comes from the #1268 adjudication.

In every one of these paths a pull request that was found keeps its number and
URL. The report never shows `#0`, `#null` or `null — null` for it:

- step-22b assigns `PR_URL` and `PR_NUMBER` from the scope helper's reply only
  when the new value is valid (a `github.com` pull request URL, a number
  matching `^[1-9][0-9]{0,9}$`). Otherwise it keeps the value it had;
- `workflow_terminal_evidence.sh` fills a missing `number` or `url` on the
  run's own pull request from the run's `pr_number` and `pr_url` before it
  removes duplicates and writes the verdict. The number comes from `pr_number`,
  or from `pr_url` when that is a complete `github.com` pull request URL.
  Entries from `gh pr list` are not changed.

Example: a run with an empty `base_branch` and no `base_branch` in its publish
result recorded `https://github.com/octo/example/pull/4242`. The pull request exists and
is open, but `gh pr view` returns no `baseRefName`. Step-22b warns, uses
`main` from `origin/HEAD`, and calls `workflow_pr_scope.sh --head <branch> --base main`.
The pull request's live base is `main`, so the match confirms the
`EXISTING_OPEN_PR` state the publish evidence set. An excerpt of the output,
with stderr and stdout interleaved and other lines left out:

```text
WARNING: workflow_final_status.sh: the pull request's base could not be read; reporting against the default branch main
terminal_success=true
terminal_state=EXISTING_OPEN_PR
terminal_status=EXISTING_OPEN_PR
PR Status:
=== PR: https://github.com/octo/example/pull/4242 ===
```

If the live base were `dev` instead, the scoped match would miss and the
#1268 adjudication would decide the terminal state.

Example: the repository's default branch on GitHub is `main`, `origin/dev`
exists, and the run is started with `-c base_branch=dev`. Step-16 opens
`feat/x → dev` and records `base_branch: "dev"`. Step-21 expects `dev`, finds
the pull request with `workflow_pr_scope.sh --base dev`, and runs
`gh pr ready`. Run again with `base_branch` empty against a pull request whose
base is `dev` and whose publish result has no `base_branch`: step-21 reads
`baseRefName` with `gh pr view`, gets `dev`, checks `origin/dev`, and marks the
same pull request ready. Run a third time with `-c base_branch=main` against
the same pull request: rule 1 gives `main`, the scoped lookup finds no pull
request from `feat/x` into `main`, step-21 ends in `no_scoped_pr`, and
`gh pr ready` is not called.

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

## How the inputs reach each phase

`default-workflow` runs its phases as sub-recipes in the same runner process.
The runner (`recipe-runner-rs`) builds each sub-recipe's context from the
sub-recipe's own `context:` defaults, then lays the parent's whole context
over them, then applies the calling step's `context:` overrides. When the
sub-recipe finishes, its context is merged back into the parent's. Every key
in the result is exported to bash steps as `RECIPE_VAR_<key>`, whether or not
the sub-recipe declares it.

A recipe declaration does not filter what a parent passes. Each recipe that
reads one of these inputs still declares it, with an empty or `"false"`
default, for two reasons:

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
[Where the inputs apply](#where-the-inputs-apply).

Every bundled recipe on a path to `default-workflow` declares
`base_branch: ""` and passes `base_branch: "{{base_branch}}"` in the
`context:` of each call to `default-workflow`, `autodrive-build`,
`auto-drive-to-merge`, `quick-change`, `smart-classify-route` or
`smart-execute-routing`:

| Recipe | Calls that pass `base_branch` |
| ------ | ----------------------------- |
| `auto-drive-to-merge` | the build call to `autodrive-build` |
| `autodrive-build` | step-02, the call to `default-workflow` |
| `auto-drive-light` | `light-escalate`, the call to `auto-drive-to-merge` |
| `quick-change` | `quick-change-handover`, the call to `default-workflow` |
| `smart-orchestrator` | the calls to `smart-classify-route` and `smart-execute-routing` |
| `smart-classify-route` | none; it declares the key only |
| `smart-execute-routing` | all three calls to `default-workflow`, and `execute-single-round-1-quick-change`, the call to `quick-change` |
| `quality-loop` | its call to `default-workflow` |
| `oxidizer-workflow` | all thirteen calls to `default-workflow` |

In-process, the whole-context merge would deliver `base_branch` without these
entries. They are needed when a phase runs as a separate
`amplihack recipe run`: that process sees the parent's value only as an
inherited `RECIPE_VAR_base_branch`, and the child's own `base_branch: ""`
declaration replaces it (#1561), so without `{{base_branch}}` in the calling
step the child would run with an empty base. The explicit entry also keeps the
value from depending on which keys some other step merged back into the
parent's context. (The older text in
[Worktree setup propagation](worktree-setup-propagation.md#the-propagation-chain)
says only declared keys are merged; that describes the earlier Python runner.
`recipe-runner-rs` merges every key.)

The runner reads a step's values only from its `context:` key. The thirteen
`oxidizer-workflow` calls to `default-workflow` used to name that map
`sub_context:`, which the runner does not know and skips with a warning, so
none of their values, the phase's own `task_description` included, reached
`default-workflow`. They now use `context:`.

`workflow-finalize`, `workflow-terminal-state` and
`workflow-terminal-adjudication` also declare `forbidden_base_branches: ""`,
because their helpers check a base read from the pull request against it.

So `amplihack recipe run auto-drive-to-merge -c base_branch=dev` reaches
step-04, step-16 and finalize of the nested `default-workflow` as `dev`, and
the same holds for the other recipes above. `{{base_branch}}` appears only in
`context:` maps, never in `command:` or bash text. The only clears are the two
`workflow-worktree` calls described in
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
opens no pull request. On that path `light-escalate` passes the caller's
`base_branch` to `auto-drive-to-merge`. After a new change, `light-worktree`
has cleared it, so escalation sees an empty base, which is correct: the
`quick/` branch was cut from `origin/HEAD`, and finalize takes the base from
the pull request itself ([How finalize finds the base](#how-finalize-finds-the-base)).

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
| `delivery_run_base EXPLICIT RECORDED PR_REF REPO BRANCH FORBIDDEN` | the finalize helpers below, the terminal-state probe | Prints the base the run's pull request has, by the rules in [How finalize finds the base](#how-finalize-finds-the-base), without `origin/`. On failure it writes the reason to stderr, with names quoted by `printf '%q'`, and returns 2 when rule 3 could not read the base (`gh pr view` failed, returned something other than JSON, or returned no `baseRefName`), or 1 for any other failure, including a base that `delivery_base_ref` refuses. |
| `delivery_default_base` | `delivery_run_base` rule 5, step-22b's fallback | Prints the first of `origin/HEAD`'s target, `origin/main`, `origin/master` and `origin/develop` that resolves to a commit, without `origin/`. Reads local refs only and never calls `gh`. Returns 1 with no output when none resolves. It does not check `forbidden_base_branches`; step-22b checks its result with `delivery_base_ref`. |

None of them read `RECIPE_VAR_*`. Each recipe passes the values it has
declared in its own `context` (#1561). `delivery_run_base` makes at most one
`gh pr view` call and never retries.

The finalize helpers take the base the same way, as an argument. A set
`--base` is never replaced. An empty or absent `--base` uses
`delivery_run_base`, which falls back to discovery only when the run recorded
no pull request. `workflow_final_status.sh` alone also falls back when
`delivery_run_base` returns 2, as described in
[When step-22b cannot read the base](#when-step-22b-cannot-read-the-base):

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
cargo test -p amplihack --test integration workflow_finalize_resilience
bash tests/issue_1596_final_status_branch_pr_lookup.sh
```

The shell test runs step-22b with a fake `gh` whose `gh pr view` returns
`number`, `url`, `title`, `baseRefName`, `headRefName`, `createdAt`,
`isCrossRepository` and `state`, as the real `gh pr view --json` does. Two
switches, read only by the fake, remove fields: `FAKE_GH_NO_BASE=1` leaves out
`baseRefName`, and `FAKE_GH_SPARSE_VIEW=1` leaves out `number`, `url` and
`title`. The cases for
[When step-22b cannot read the base](#when-step-22b-cannot-read-the-base)
check that an unreadable base gives one warning and `--base main` for the
scope helper, that a sparse reply still prints `#4242` and the pull request
URL, and that a pull request in another repository, a forbidden default
branch, or a live `baseRefName` in `forbidden_base_branches` gets no fallback
and no fallback warning. Two cases check that a fallback base never confirms
a terminal no-diff, and that the same set-up with `--base main` still does.
A unit case checks that `delivery_run_base` returns 2
for an unreadable base and 1 for a refused one. None of them may print `#0`, `#null` or
`null — null`.

The finalize tests use a fake `gh` and a local git repository whose
`origin/HEAD` is `main`, with a real `origin/dev` and pull request 7
(`feat/x → dev`). They cover `base_branch=dev`, an empty `base_branch` with
the base recorded in the publish result, an empty `base_branch` with no
recorded base (one `gh pr view` call), and the cases that must fail closed: an
explicit or recorded `main`, a recorded base with no `origin/<base>`, a base on
the forbidden list, a base that exists only as a local ref named
`origin/dev`, a malformed `pr_number`, and a failed `gh pr view` while
`origin/HEAD` names the right branch. `gh pr ready` is never called in those
cases. Each test sets every `RECIPE_VAR_*` and `WORKFLOW_*` variable the
helpers read, so it does not depend on its environment.

`every_finalize_recipe_declares_base_branch` and
`every_caller_passes_base_branch_to_default_workflow` parse the recipes listed
in [How the inputs reach each phase](#how-the-inputs-reach-each-phase) and
check the declarations and the `{{base_branch}}` pass-through, allowing only
the two documented clears.

## Related documentation

- [Tracking Issue Selection Reference](workflow-tracking-issue-selection.md) - how step-03 chooses the issue when `strict_delivery` is off
- [Issue Deduplication Reference](issue-dedup.md) - the issue claim check
- [Workflow GitHub Repo Identity](workflow-github-repo-identity.md) - the `origin` parser publication uses
