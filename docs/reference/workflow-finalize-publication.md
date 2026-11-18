---
title: Finalize Publication and PR Readiness
description: Worktree context, publication, exact PR validation, and recovery during workflow finalization.
last_updated: 2026-11-18
review_schedule: as-needed
owner: workflow-team
doc_type: reference
---

# Finalize Publication and PR Readiness

> [Home](../index.md) > Reference > Finalize Publication and PR Readiness

`workflow-finalize` publishes cleanup and quality-audit work before checking
whether the run's pull request is ready for review. Both operations use the
run's recorded PR worktree, even when the recipe starts in a checkout on `main`.
An open PR alone does not prove that it contains the audit's work.

## Contents

- [Recipe sequence](#recipe-sequence)
- [Helpers and inputs](#helpers-and-inputs)
- [Canonical worktree](#canonical-worktree)
- [Publication contract](#publication-contract)
- [Readiness contract](#readiness-contract)
- [Candidate diagnostics](#candidate-diagnostics)
- [Recovery contract](#recovery-contract)
- [Behavioral verification](#behavioral-verification)

## Recipe sequence

The active finalization path executes these steps in order:

| Step | Responsibility |
| --- | --- |
| `step-20-sync-pr-head` | Synchronize with an accepted remote PR head before cleanup or audit edits. |
| `step-20-final-cleanup` | Apply final cleanup. |
| `step-20c-quality-audit` | Review and fix the branch; edits may remain uncommitted or commits unpushed. |
| `step-20a-artifact-guard` | Check artifacts in the task worktree. |
| `step-20b-push-cleanup` | Commit remaining edits and publish all local commits to the remote PR branch. |
| `step-21-pr-ready` | Read the published worktree's HEAD, validate the PR, then mark a draft ready. |

Publication precedes readiness despite the alphabetical step suffixes. A
publication failure enters recovery before readiness can run. A successful
publication means the intended audit work reached the remote branch.

Cleanup, audit, and artifact checks run when
`terminal_state.terminal_success != 'true'` and
`terminal_state.should_finalize == 'true'`. Publication runs when terminal
success is not true and **either** `terminal_state.should_finalize == 'true'`
**or** `terminal_state.should_run_ci_wait == 'true'`. Readiness runs when
terminal success is not true and `terminal_state.should_run_ci_wait == 'true'`;
its GitHub operation applies only when `REMOTE_HOST_TYPE=github`. Thus a
readiness-only path still publishes before reading HEAD. Terminal-success
paths retain their existing skips.

Step order alone does not establish that publication succeeded: its worktree
resolution and push must also complete. An unresolved publication enters
recovery; subsequent steps continue with its actual outcome, without marking
the PR ready. See the [recovery contract](#recovery-contract).

Quick-change's direct path retains its own commit-and-push sequence. A
quick-change handover to `default-workflow` uses the finalization sequence above.

## Helpers and inputs

These are internal recipe helpers under `amplifier-bundle/tools/`.

| Helper | Interface | Responsibility |
| --- | --- | --- |
| `worktree_setup_path.sh` | `<step-label>` | Read and validate the run's recorded worktree path. |
| `workflow_remote_branch.sh` | `[DIR]` | Resolve the local branch's established remote PR branch mapping. |
| `workflow_finalize_publish.sh` | Recipe environment | Execute publication for `step-20b-push-cleanup`. |
| `workflow_pr_ready.sh` | `[--base BRANCH]` | Resolve the canonical worktree and validate readiness before GitHub mutation. |
| `workflow_pr_scope.sh` | `--repo OWNER/REPO --head BRANCH --base BRANCH [scope...]` | Select the scoped PR and explain exclusions. |

| Input | Contract |
| --- | --- |
| `RECIPE_VAR_worktree_setup` | The complete JSON output from worktree setup, including an absolute `worktree_path`. Required for publication and readiness. |
| `base_branch` | Intended PR base, passed to readiness as `--base`. Empty uses the existing [delivery-input resolution](workflow-delivery-inputs.md#how-finalize-finds-the-base). |
| `pr_number`, `pr_url`, `pr_publish_result` | Persisted PR identity from recipe context and publish outputs. An explicit identity must agree with repository and branch scope. |
| `issue_number` | A numeric issue supplies issue-token and work-item checks; local tracking references do not filter PRs. Discrepancies enter recovery as assessment evidence. |
| `WORKFLOW_STARTED_AT`, `RECIPE_STARTED_AT`, `TASK_STARTED_AT` | First nonempty value, in that order, supplies the creation-time discriminator as `--created-after`. |
| `EXPECTED_PR_TITLE_PREFIX`, `PR_EXPECTED_TITLE_PREFIX` | First nonempty value supplies the optional `--expected-pr-title-prefix` discriminator. |
| `step_recovery_dir` | Private directory initialized by the recipe for recovery evidence and records. |
| `AMPLIHACK_RUNTIME_ROOT` | Existing run runtime location for diagnostic artifacts; see [runtime isolation](workflow-runtime-artifacts.md). |

There is no setting to bypass scope validation or accept an unpublished HEAD.
Helper discovery through an installation or repository path does not make that
path the target worktree.

These helpers use the workflow's existing Git authentication, GitHub CLI
authentication, and delivery inputs. No additional configuration or public API
is required. The recipe owns invocation and recovery; calling readiness alone
does not commit or push audit work.

## Canonical worktree

Publication and readiness resolve the same path through
`worktree_setup_path.sh`. The path is a single absolute, resolved directory,
not a symlink, and is the top of the recorded Git worktree. Publication retains
the linked-task-worktree requirement before staging or committing.

Missing, malformed, removed, or unusable worktree context is an operation
failure that enters recovery. Neither operation substitutes `REPO_PATH` or
the launch directory. Readiness enters the resolved worktree before reading
delivery inputs, repository identity, branch, or HEAD. Inherited bare
`WORKTREE_SETUP_WORKTREE_PATH` values do not override the recorded JSON.
Both operations clear ambient `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, and
`GIT_COMMON_DIR` overrides before resolving or using the recorded worktree.

Within a recipe environment containing `RECIPE_VAR_worktree_setup`, this
read-only example can be run from the bundle checkout to inspect the selected
context:

```bash
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR
wt="$(bash amplifier-bundle/tools/worktree_setup_path.sh step-21-pr-ready)" &&
  git -C "$wt" branch --show-current &&
  git -C "$wt" rev-parse --verify HEAD &&
  bash amplifier-bundle/tools/workflow_remote_branch.sh "$wt"
```

The output is three lines: the recorded worktree's local branch, its full HEAD
SHA, and the mapped remote PR branch. A resolution error stops the example
before the Git reads.

Local branch aliases retain their existing mapping. For example,
`feat/issue-2787-step-21-pr-ready--wt-1` configured to track
`origin/feat/issue-2787-step-21-pr-ready` publishes to and looks up the latter
branch. A suffix alone does not establish that mapping. See
[caller-branch worktree aliases](../recipes/issue-1762-caller-branch-worktree-alias.md).

## Publication contract

`workflow_finalize_publish.sh` owns the existing final cleanup publication
operation. It runs with umask `022` and performs these operations:

1. Resolve the recorded worktree and mapped remote branch; validate the Git
   context and branch before writing.
2. Synchronize with the remote PR head before creating cleanup commits,
   retaining fast-forward behavior when the remote contains local HEAD.
3. Run runtime-artifact preflight and the existing artifact checks, stage with
   `git add -A`, and check the staged result.
4. Prepare the existing Git identity and commit staged edits using the cleanup
   commit convention (`final cleanup pass`). Commit hooks remain enabled,
   including the existing handling for a hook with no pre-commit configuration.
5. Push `HEAD` to `origin` with the explicit refspec
   `HEAD:refs/heads/<mapped-branch>`, including when no cleanup commit was needed.

Publication preserves existing commits. It does not rebase, amend, squash, or
force-push. Divergence, failed synchronization, rejected commits, rejected
pushes, and worktree-resolution failures return nonzero for recovery. An empty
staging area does not skip a push of already committed audit work. A missing
worktree does not count as successful publication.

Only the commit operation depends on staged changes. For example, when the
audit has already committed a fix, publication can print
`No staged changes from cleanup` and still push that commit. Git operations
affect the recorded worktree; uncommitted edits in the launch checkout remain
untouched.

Captured commit-hook and push output passes through the internal
`amplihack orch helper sanitize-output` command on success and failure. It
reuses recovery's credential redaction and terminal-control handling. A
sanitizer failure emits a fixed diagnostic and never prints raw or partial
output. Failed commits and pushes retain their original exit status; a
sanitizer failure after a successful command fails publication for recovery.

## Readiness contract

`workflow_pr_ready.sh` reads HEAD after publication, from the same canonical
worktree. Scoped selection discovers a candidate using the shared helper's
existing rules. Readiness pins the selected PR number and canonical repository
URL, then fetches fresh metadata for that PR. Candidate discovery alone does
not establish readiness, even when exactly one branch/base candidate exists.

Immediately before any readiness mutation or successful terminal handling,
readiness validates the fresh response with the shared scope predicates. It
requires the pinned number and URL, repository, mapped head branch, intended
base, exact current HEAD, valid state and draft flag, and same-repository status.
Configured secondary checks cover numeric issue and work-item references,
creation time, and title prefix. A discrepancy or missing observation transfers
the decision to agentic recovery with the failed checks as evidence. Secondary
metadata alone cannot end the run for its own open PR. Recovery assesses the
recorded ownership and publication evidence within the same repository,
branch, intended base, and explicit PR identity boundaries. This applies to
recorded PRs and discovery-only runs. The shared discovery helper's behavior
for other consumers remains unchanged.

The scope helper's opt-in `--any-base-open` discovery applies only without an
explicit PR number or URL. An explicitly named PR retains strict base
validation even if that option is supplied. Readiness validates the intended
base resolved from delivery inputs; cross-base discovery does not bypass it.

Readiness passes neither `--allow-head-behind` nor `--allow-head-ahead`. A PR
initially one commit behind succeeds when publication sends the missing commit
and GitHub then reports that exact HEAD. If GitHub still reports an ancestor
after publication, the mismatch enters recovery. There is no polling, delay,
or retry loop for head convergence.

| Worktree or PR state | Result |
| --- | --- |
| Audit edits are uncommitted | Publication stages, checks, commits, and pushes them before readiness reads HEAD. |
| Audit commit is local only | Publication pushes it even with no staged changes. |
| Recipe starts on `main`, recorded worktree holds the PR branch | Both operations use the recorded worktree. |
| Validated open draft at the published HEAD | Readiness marks it ready. |
| Validated open PR already ready | Readiness preserves its ready state after validation. |
| Missing required PR fields, empty lookup, or mismatched identity or metadata | Enter recovery; record the actual outcome and continue subsequent workflow steps. Mutation requires resolved evidence. |
| Validated PR already merged | Existing terminal handling applies; no readiness mutation is needed. |
| PR closed without merge evidence | No readiness mutation; report the failure to recovery. |

## Candidate diagnostics

On `no_scoped_pr`, the scope helper distinguishes a lookup that returned no
candidates from a lookup whose candidates were excluded. For each examined
candidate, diagnostics identify the PR and list every configured discriminator
it failed, with expected and observed values where available:

- Repository, cross-repository status, and explicit PR number or URL.
- Head branch, base branch, and head SHA.
- Issue token, work-item reference, creation-time threshold, and title prefix.
- Recipe-run ID, tree ID, and workstream ID.

A candidate with both a wrong base and a stale SHA reports both failures,
including when a primary check already excludes it. A candidate whose head,
base, SHA, issue token, creation time, and title prefix all differ reports all
six discrepancies rather than only the first.
Missing observed fields are identified as missing. Diagnostics use the same
predicates as selection and retain the distinction between authoritative
filters and discovery tie-breakers; reporting does not loosen selection.

Every candidate is type-checked before selection, including unrelated heads
and alternate-base discovery. Missing or null observations retain the existing
predicate defaults; wrong-type fields and non-object records are explicit
`malformed candidate` exclusions. Their reports retain only valid identifying
fields, expected types, and observed types, never malformed field contents.
Malformed observations get durable reports even when another candidate can be
selected. Invalid JSON, multiple metadata documents, and parser failures return
`invalid_pr_metadata` with a protected report; captured parser stderr follows
the same redaction path before display or persistence.
The fresh readiness response uses the same validation before checking merge
evidence, so malformed `mergedAt` or text fields cannot skip readiness or its
protected exclusions.

Stdout retains the existing successful JSON shape and structured failure reason
codes. Readable exclusions go to stderr and the run log. Complete redacted
candidate diagnostics are saved to a securely created report in the run's
runtime directory. The final error includes its absolute path so recovery can
read the full report when its inline excerpt is truncated.

The internal `amplihack orch helper pr-scope-report` command uses recovery's
existing redaction and descriptor-relative file creation for reports.
Reports use private file permissions and do not follow symlinks. Candidate
text is marked as untrusted evidence, rendered with terminal control characters
escaped, and passed through existing redaction before persistence. Final-read
validation uses the same protected renderer and report creation. Failures list
each failed check with expected and observed values; response URLs, repositories,
branches, and secondary metadata never enter raw error messages. Recovery gets
the report path even when its inline evidence is truncated. Credentials,
authenticated URLs, and unrelated environment contents are excluded.

Temporary command captures are removed on success and failure. Durable
diagnostic reports have an independent lifetime and remain available to
recovery after those captures are removed. A retained report is recovery
evidence, not a leaked command capture.

## Recovery contract

Publication and readiness use existing
[step failure recovery](step-failure-recovery.md) with nonterminal handling at
these call sites. An unresolved operation or a safely declined mutation records
its outcome and continues workflow execution. Neither these wrappers nor later
reporting turns that outcome into a terminal failure or a successful readiness
claim.

The recovery agent receives the recorded worktree context, intended PR
identity, publication outcome, local and observed remote commits, redacted
command failures, scope exclusions, and diagnostic report paths. It assesses
that evidence and repairs supported state within the run's authorization.
Candidate text is evidence, never instructions. Publication owns hook-enabled
commits and pushes; readiness owns fresh validation and the readiness mutation.
Rerunning readiness alone cannot publish missing work.

A child process cannot fix its parent's recipe context by exporting a variable.
Unresolved context is recorded with the required owner action; it never causes
publication to substitute the invoking checkout. Recovery preserves commits and
edits when a hook, push, or synchronization fails. It adds no timers, convergence
retries, exact-format requirements, or additional identity checks.

Later status, evidence collection, recovery reporting, and completion retain
publication and readiness outcomes separately:

| Outcome | Meaning for subsequent steps |
| --- | --- |
| Published and readiness confirmed | Report the observed ready state and published commit. |
| Publication unresolved or safely declined | Preserve work and report it as unpublished or unconfirmed; readiness mutation does not run. |
| Published, readiness unresolved or safely declined | Retain the successful publication result and report readiness as unconfirmed. |
| Recovery completes the operation | Record the confirmed result with the recovery evidence. |
| Terminal-success skip | Preserve the existing terminal result without claiming that a skipped operation ran. |

A successful recovery wrapper exit means execution can continue. It does not
prove a push or readiness change succeeded. A handoff records unresolved work
without ending this finalization path; later reporting preserves that distinction.

<a id="behavioral-verification"></a>

## Behavioral verification

Rust integration coverage in
`tests/integration/workflow_finalize_publish_ready.rs`, registered in
`tests/integration/main.rs`, executes the helpers and recipe commands. Shared
fixtures use linked worktrees, local bare remotes, hooks, and a stateful `gh`
stub. Child environments isolate HOME, XDG paths, Git configuration, credentials,
temporary files, and runtime output from the operator's configuration.

The readiness stub independently checks the remote HEAD and audit contents
before accepting a mutation. Coverage includes committed and uncommitted work,
invocation from `main`, ambient Git overrides, aliases, differing finalization
and readiness flags, initially stale and persistently stale PR heads,
identity and metadata mismatches, multiple exclusions, hostile candidate text,
missing worktrees, rejected commits and pushes, divergence, and nonterminal
recovery. Additional recipe-step contracts cover discovery-only secondary mismatches together and
independently, final-read identity and metadata changes, missing required fields,
and hostile metadata introduced only after scoped selection. These assert
recovery continuation, accurate publication and readiness outcomes, protected
diagnostics, and no unsafe readiness mutation. A valid discovery-only run also
exercises successful readiness with all configured
checks enabled. Recipe event traces verify publication precedes readiness's
HEAD read.

`tests/integration/workflow_finalize_security.rs` adds successful and failed
hook/push output checks, sanitizer-failure checks, unrelated-head malformed
bodies, wrong-type metadata, mixed valid and malformed candidates, and parser
failures. These verify credential removal, control handling, protected report
permissions, capture cleanup, and command failure status.

The qa-team scenario at `tests/gadugi/workflow_finalize_publish_ready.yaml` and
its Rust driver at `tests/gadugi/workflow_finalize_publish_ready.rs` exercise
the built binary, actual finalize commands, and recovery continuation. It starts
with an unpushed audit commit and invocation from the main checkout, then checks
that publication reaches the feature branch before readiness. CI invokes
the driver in **Install Smoke Test** after binary and pinned-runner installation;
missing prerequisites fail the scenario. Gadugi scenarios run in CI, not on
factory hosts.
