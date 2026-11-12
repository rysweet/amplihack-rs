---
title: Auto Drive To Merge Reference
description: responsibilities, authorization, recovery, completion outcomes, and target compatibility for auto-drive-to-merge.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto Drive To Merge Reference

The recipes use agent responsibilities, runner-captured authorization and portable Rust evidence records. The controlled validation covers record handling and runner boundaries; live agent recovery and the six OS/runtime cells need separate execution evidence.

`auto-drive-to-merge` builds a pull request, reviews and repairs it with
`crusty-old-engineer`, verifies all merge-ready criteria, and merges when
authorized. With `no_merge=true`, it leaves the PR open and reports readiness
only when every required criterion has valid evidence.

The target is the same recipe on Linux and macOS with Claude, Codex, and Copilot;
this is not yet verified compatibility. See the [target matrix](#target-compatibility-matrix).
Steps define responsibilities and completion outcomes. Commands in prompts
are suggestions: agents assess results and recover through available
alternatives within the existing loop-health and budget rules.

## Contents

- [Current behavior and authority](#current-behavior-and-authority)
- [Responsibilities](#responsibilities)
- [Configuration](#configuration)
- [Authorization](#authorization)
- [PR head and worktree resolution](#pr-head-and-worktree-resolution)
- [Crusty review](#crusty-review)
- [Crusty completion decision contract](autodrive-crusty-completion.md)
- [Readiness and finalization](#readiness-and-finalization)
- [Loop health and budgets](#loop-health-and-budgets)
- [Resumability](#resumability)
- [Trusted execution and portability](#trusted-execution-and-portability)
- [Target compatibility matrix](#target-compatibility-matrix)

## Current behavior and authority

The executable [composer](../../amplifier-bundle/recipes/auto-drive-to-merge.yaml) defaults outer `no_merge` to `false`. Its build always passes `no_merge=true` to default-workflow. The runner captures the intended action before children execute, pins a private launch copy, and propagates literal restrictions through shared loops and light escalation. Passing `no_merge=true` prevents an authorized merge throughout that invocation; dry-run additionally suppresses follow-up issue creation.

<a id="merges-outside-the-gate-are-refused-1551"></a>

The shell gate, process-identity shim and hook restriction are retired. The bundled [auto-drive skill](../../amplifier-bundle/skills/auto-drive-to-merge/SKILL.md) describes the new workflow; the trusted [merge-ready skill](../../amplifier-bundle/skills/merge-ready/SKILL.md) remains authoritative for all 13 criteria and established interpretations. Criteria and helpers never fall back to PR-authored copies. Prompts and private records are workflow safeguards, not an OS security boundary.

### How the merge round applies merge-ready

Inside auto-drive, criterion 3 substitutes accepted crusty convergence
(an evidence-valid `CLEAN` or permitted `MINOR_ONLY` accepted by the
completion dispatcher, with required follow-ups filed or verified)
for the separate quality audit. There is **no minimum round count**.
Standalone `/merge-ready` still requires at least three SEEK → VALIDATE → FIX
cycles, continuing until clean. The description template's three cycle rows
apply to that standalone audit; auto-drive records its actual accepted crusty
rounds, reviewed head, verdict, and follow-up evidence instead of inventing
cycles. See [all criterion interpretations](auto-drive-evidence.md#authority-and-criteria).

## Responsibilities

| Responsibility | Completion outcome |
| --- | --- |
| Prepare or build | Establish PR identity, platform state, head, base, and linked worktree. Run `default-workflow` with `no_merge=true` when a build is needed. End with an open PR. |
| Crusty review and repair | Repair concerns, commit and push, and re-review until an evidence-valid `CLEAN` or `MINOR_ONLY` round completes the review objective; the Rust dispatcher accepts completion without a loop-health evaluator call. Unresolved concerns go to loop-health evaluation. Bind review to the reviewed head. |
| Collect and assess evidence | Apply the trusted merge-ready skill and description template. Collect concrete evidence for all 13 criteria; repair blockers and repeat until readiness and loop-health `DONE` agree. |
| Authorized finalization | Refresh mutable facts, validate evidence, and carry out only the captured action. With merging disabled, observe and report the open PR. For a merge, protect the expected head and observe platform confirmation. |
| Report | Reduce accepted records into readiness, requested action, observed platform state, blockers, failures and recovery, and stop reason. |

An open PR for the selected branch skips the build. A PR confirmed merged at
entry skips work and is reported as already merged. An unexpected merge during
build or review is a workflow violation, not successful finalization.

### Finalization hand-off from phase 1 (#1714)

Build-phase merge prohibition and final merge authorization are separate.
A build failure neither authorizes merging nor establishes readiness.

### Which states are handed on

`BLOCKED_CI`, or `FAILED_IMPLEMENTATION` with a confirmed open PR, hands that
PR to review and readiness work. A failure before a PR exists remains
incomplete. Receiving steps preserve the cause and resolve outstanding work
before claiming readiness.

## Configuration

Pass context with `amplihack recipe run auto-drive-to-merge -c key=value`.
See [verify a PR without merging](../howto/verify-pr-with-auto-drive.md) for
commands and expected outcomes.

| Context key | Default | Meaning |
| --- | --- | --- |
| `task_description` | empty | Work to build when no existing PR is selected; last fallback for a task reference. |
| `requirements` | empty | Additional build requirements. |
| `repo_path` | `.` | Repository used to establish PR identity and linked worktrees. |
| `pr_number` | empty | Explicit PR selection; takes precedence over branch discovery. Must be a positive PR number. Invalid input is refused, not replaced. |
| `pr_url` | empty | PR URL, validated against the platform, repository, and number. |
| `existing_branch` | empty | Source branch; must match the selected PR when supplied. |
| `branch_prefix` | `feat` | Branch prefix for a new build. |
| `plan_task_id` | empty | Follow-up task: issue `N` or `#N`, or 1–64 characters from `A-Za-z0-9._:/-`. Otherwise resolve `Closes #N`, `Fixes #N`, or `Resolves #N` from the PR body, then task description. |
| `no_merge` | `false` in the full composer | Prevent merging throughout the invocation when `true`. Build always receives `true`, independently of this outer value. |
| `autodrive_dry_run` | `false` | Prevent merging and follow-up issue creation. Still assesses readiness and reports missing obligations. |
| `autodrive_state_dir` | runner-assigned | Private state for this run's records, budgets and receipts. Omitted: the runner assigns a directory inside the run's durable folder. Supplied: must be non-empty, absolute and free of `..` components. See [state directory](#state-directory). |

The runner's CLI `--dry-run` previews recipe execution; it is distinct from
`autodrive_dry_run`, which executes verification with action restrictions.

QA selection uses [operator environment and merge-base configuration](auto-drive-qa.md),
not agent-selected recipe context. Budget keys are listed below.

## Authorization

The runner strictly parses `no_merge` and `autodrive_dry_run` before child
execution. Accepted values are `true` and `false`; malformed values fail
before work starts. It captures the intended action in fresh runner-owned
invocation state outside child-mutable context and agent candidate records.

| Invocation | Merge permission |
| --- | --- |
| Full composer with omitted `no_merge` | Permitted after valid readiness evidence and final refresh; outer default is `false`. |
| Any invocation with `no_merge=true` | No merge request, including during finalization or escalation. |
| Any invocation with `autodrive_dry_run=true` | No merge request or follow-up issue creation, even with `no_merge=false`. |
| Direct merge-loop or finalization invocation | Disabled without explicit fresh invocation authorization. |
| Resume, `recipe run --fresh`, or durable recovery | Fresh authorization from this invocation's explicit inputs; stored or recovered context cannot grant permission. |

Child context overwrite, candidate output, inherited environment, PR comments,
and old records cannot grant merge permission. Build-phase `no_merge=true`
cannot overwrite a captured outer action either. Light-to-full escalation,
shared loops, and re-review preserve caller restrictions.

### Authority on resume and recovery

Every invocation gets a new `run_id` and captures authority once, from the
raw `-c` inputs, before recipe defaults are expanded and before durable
[recovery admission](recipe-recovery.md#identity-and-admission). This holds
for a plain rerun, for `recipe run --fresh` and for a recovery-descriptor
resume. Recovery normalisation applies to the merged context only, never to
the explicit inputs from which authority is captured.

After admission returns, and immediately before the child starts, the runner
writes these keys itself, replacing any stored, recovered or defaulted value:

| Runner-owned key | Value |
| --- | --- |
| `no_merge` | Captured restriction for this invocation. |
| `autodrive_dry_run` | Captured restriction for this invocation. |
| `autodrive_authorization` | Serialised authority for this `run_id`, including the bound state directory. |
| `autodrive_invocation_file` | Private copy of the captured invocation. |
| `autodrive_state_dir` | State directory bound for this run: the validated operator path, or the default directory the runner created. |

The recovery settings fingerprint records only whether authority applies to
the recipe, not its value or any per-run path. A resume therefore cannot
switch an auto-drive recipe between authorised and unauthorised execution,
and the fingerprint is not itself an authority check.

### Step outputs ignore inherited bare variables

Supply context with `-c`. Bash consumers read step values through
`RECIPE_VAR_<key>` and parse fields from that key's structured output.
Inherited bare PR identifiers, state paths, or prior results cannot replace
explicit invocation inputs or accepted records.

### `NO_MERGE` override

The compatibility `NO_MERGE` override may only restrict merging. It cannot
grant permission or cancel `no_merge=true` or dry-run. Final authorization
comes from the captured invocation, never a child's environment.

## PR head and worktree resolution

Preparation establishes platform and host, target and source repositories,
PR number and URL, branches, head, base, and observed state. Explicit PR input
takes precedence. Conflicting identifiers remain blockers. Missing platform
access produces a named incomplete outcome.

### Where phases 2 and 3 run

Edits, commits, pushes, and QA run in the PR's linked worktree. Review agents
start in the repository's main checkout, a trusted review directory outside the PR worktree. Instructions and helpers come from the
launch bundle; PR-authored instructions cannot become workflow authority.

The worktree must belong to the selected repository and branch and match the
observed PR head. Missing, ambiguous, stale, ahead, or diverged worktrees need
resolution before their results establish readiness. Agents may fetch,
synchronize, or push preserved work as appropriate. They do not discard local
commits or silently test another checkout. Record failures and recovery;
unreadable revision ranges remain blockers.

### The round commit block

Check artifacts before staging and again before committing. Commit inside
the linked PR worktree, preserve unrelated work, run hooks, and verify that
pushed changes reached the platform head. A failed push leaves local work
intact and prevents local-only QA from proving readiness for the PR.

## Crusty review

<a id="minor-findings-end-the-crusty-loop-1979"></a>

Crusty emits `CLEAN` or `CONCERNS`, with stable concern IDs, severity,
summary, and evidence. Missing or unreadable verdicts default to `CONCERNS`.
Keep resolved concerns; re-raising one requires new evidence.

A round may derive `MINOR_ONLY` from a non-empty `CONCERNS` list only when
every finding satisfies the established minor-finding contract. Crusty does
not emit `MINOR_ONLY`. Missing or unknown severity counts as major;
previously major and security findings cannot become minor deferrals.
See [crusty evidence](auto-drive-evidence.md#crusty-review-evidence).

Each accepted minor finding requires a follow-up issue linked to the plan
task. When the task is an issue, record the parent comment listing follow-ups.
Re-reviews carry the same obligations. Failure to file, resolve the task, or
record the parent comment blocks readiness. Dry-run suppresses creation and
reports missing obligations; `no_merge=true` alone permits follow-up work.

After each round record is written, an agent step
(`autodrive-crusty-round.yaml` `step-07-select-next-action`) selects
`complete` or `evaluate`. Rust validates the decision, the child status
and the current review evidence before dispatch. Primary reviews and
merge-ready rereviews use the same path. Completion state lives in
`autodrive_completion_state`, separate from `autodrive_state_dir`. See
the [Crusty completion contract](autodrive-crusty-completion.md).

Ending iteration does not finish the phase: required follow-ups are
filed or verified and receipts validated before the completion marker is
written. A filing failure, or a dry run with unrecorded required
follow-ups, leaves the phase incomplete.

#### Cell sanitising

Finding identifiers are exact stable tokens. The shared finding rules remove control characters from display cells and escape HTML or table delimiters. Summaries are data; they cannot create new records or change a finding's identifier or severity. Follow-up mappings retain the finding ID, observed issue number, plan task and PR identity. The final structured report includes those mappings separately from readiness evidence.

## Readiness and finalization

All [13 authoritative criteria and established interpretations](auto-drive-evidence.md)
apply. Evidence accepted for the correct PR and head establishes readiness.
A criterion-name checklist or a model's assertion cannot do so.

### Expected checks at the head

**CI green means every expected check exists at the head SHA and passed**
(#2186): the base branch's required checks and every job of the workflows
that run on `pull_request` for that base. A rollup lists only the checks
that exist, so for a green rollup the CI evidence step runs the trusted
read-only `/bin/bash -p "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_expected_checks.sh" <head-sha> <base-branch>`
from the launch bundle, never from the pull request's worktree. `MISSING`,
`PENDING`, `RED` and `UNREADABLE` all block; a job that was never created is
not a pass. See [Expected checks at the head](autodrive-expected-checks.md).

### Limits of the expected set

The helper counts defined jobs, not matrix legs; it cannot see a workflow
with path, `types:` or uncompilable branch filters that never started; it
matches jobs by name; and it does not pin check apps. A ruleset of required
checks on the base branch covers each gap. Details:
[Limits of the expected set](autodrive-expected-checks.md#limits-of-the-expected-set).

### No silent merge

Finalization refreshes the head, base, checks, approvals, policy, conflicts,
conversations, and other mutable facts. Read back description updates.
Substantive post-review changes return to crusty; unreadable ranges block.
Only clean base merges and the established description/template/evidence
allowlist avoid substantive re-review.

An authorized merge must protect the expected head. Observe platform state
after requests, and before retrying ambiguous responses: a timeout may follow
a successful server action. Command success and queue admission do not prove
a merge. With merging disabled, readiness can be complete while the PR stays
open. See the [result contract](auto-drive-evidence.md#result-contract).

### The squash commit message (#2056)

`main` squashes with `squash_merge_commit_message: COMMIT_MESSAGES`, so a
merge that supplies no message gets every commit on the branch concatenated:
each commit's trailers, a `Fixes #N` written for a stacked branch, and the
`Co-authored-by` lines GitHub writes for commit authors. A wrong trailer on a
pushed commit cannot be corrected without a force-push, which this workflow
never does. So an authorized merge supplies its own message.

The `finalize-observed-action` step sources the trusted launch-bundle
`amplifier-bundle/tools/autodrive_merge_message.sh`, never a copy from the
PR's tree, and composes the message from one `gh pr view --json title,body`
read:

| Part | Source | Refused when |
| --- | --- | --- |
| Subject | `autodrive_merge_subject`: the title with C0, DEL and C1 controls removed by code point, then ` (#N)` unless the title already ends with this PR's suffix | the title is absent, null, not a string, blank, or over 1024 characters |
| Body | `autodrive_merge_body`: the description up to the first line starting `<!-- merge-ready-report` or `## Merge readiness`, so the report never lands on `main`; `MERGE_REPORT_CUT` says whether one was cut | the `body` key is absent, not a string or null, or over 65,536 bytes |

The body goes to `gh` on standard input from the shell's memory, never as a
path a same-user process could rewrite between a check and the read:

```bash
printf '%s' "$MERGE_BODY" | gh pr merge <N> --squash --match-head-commit <sha> --subject "$subject" --body-file -
```

A refusal from either function is a blocker; the step reports it and does
not fall back to the platform's default message. The step records the
subject and whether a report was cut.

Two limitations stay open. An empty description gives `gh` an empty body;
whether GitHub then falls back to its default message is unverified (#2263).
GitHub is believed, from landed commits rather than a controlled probe, to
add no `Co-authored-by` lines to a supplied body (#2263).

### Hosting calls and rate limits

The bundled [evidence contract](../../amplifier-bundle/skills/auto-drive-to-merge/evidence-contract.md)
governs calls to the hosting service:

- Use only configured authenticated access to the bound host and repository,
  with TLS verification.
- Check both transport status and API errors, including GraphQL errors in
  HTTP 200 responses.
- Pagination is complete only when every page succeeds and the final page
  reports no next page. Discard partial counts after any failed page.
- On a rate limit, a transient read failure or a refused call, the step does
  not wait or retry. It reports `incomplete` and records the cause, the
  response status, and any `Retry-After` or reset time as observations. A
  later drive picks the work up.
- Authentication, permission, validation and protection refusals are not
  retried.
- Rate-limit observations are limited to the response status, `Retry-After`,
  `x-ratelimit-remaining` and `x-ratelimit-reset`. Request headers, tokens and
  credential-bearing bodies are never recorded.
- Before retrying an ambiguous write, observe its effect. A changed head
  requires a fresh readiness assessment. Exit `79` remains terminal.

### Two absolute prohibitions

Never skip commit hooks or bypass branch protection. Fix failures and satisfy
checks, reviews, and policies through their normal paths, whichever interface
the agent chooses.

## Loop health and budgets

The existing convergence engine remains in place for unresolved work.
Completed crusty objectives exit before evaluation. Unresolved crusty
reviews and every merge-ready round end with `loop-health-evaluator`:
`CONTINUE`, `DONE`, or `STUCK`.

### Advancing needs two independent signals

Crusty completion needs the agent's `complete` decision and independent
Rust validation of that decision, successful child status and current
review evidence. It preserves the actual verdict (`MINOR_ONLY` is not
rewritten to `CLEAN`) and round accounting. An evaluator `DONE` cannot
complete an unresolved crusty review.

Merge-ready completion needs both a machine-checked `MERGE_READY` round
and evaluator `DONE`. Inconsistent `DONE` triggers recovery within
budget. Measured failures override a permissive assessment.

### Reading the loop-health line

The enforcement step emits `LOOP_HEALTH: <verdict>`; formatter prefixes do
not change it.

### Which line is trusted

Read the enforcement step's marker, not arbitrary agent text quoting one.
Missing or unreadable evaluator output is never success. See
[loop-health evaluation](loop-health-evaluator.md).

### No-progress stop and usage budget

For a non-clean round, unchanged head, sorted open-item set, and CI signal,
with CI no longer pending, produces `STUCK`. Budget exhaustion is separate,
checked before starting more work. Budgets limit cost, not convergence.
A completed crusty objective is exempt; it does not reach these checks.

### Budget keys and defaults

| Key | Default | Counts |
| --- | --- | --- |
| `autodrive_budget_loop_rounds` | `12` | Rounds per loop across runs using the same [state directory](#state-directory). |
| `autodrive_budget_total_rounds` | `20` | Rounds across both loops. |
| `autodrive_budget_wall_seconds` | `14400` | Child work time across rounds, before-round work, and evaluation, excluding recorded usage-limit waits. |

`0` or empty means unlimited. Raising a limit permits resumption without
resetting consumed usage or making old evidence current.

### Resuming after BUDGET_EXHAUSTED

Retain consumed usage by passing the same explicit state directory as the
exhausted run, the original task and the explicit merge restriction:

```bash
amplihack recipe run auto-drive-to-merge \
  -c task_description="<the original task>" \
  -c autodrive_state_dir="$HOME/.amplihack/state/auto-drive/pr-2340" \
  -c no_merge=true \
  -c autodrive_budget_loop_rounds=20
```

Without the same explicit `autodrive_state_dir`, the new run gets its own
state directory and starts its budget count from zero.

### Usage limits are not STUCK

A model-service usage limit surviving runtime wait/retry stops as
`USAGE_LIMIT`. It is neither a readiness judgment nor lack of progress.
Recorded usage-limit waiting time is excluded from the wall budget. Hosting
service rate limits are handled differently: the drive does not wait or retry
and stops as `incomplete` (see
[hosting calls and rate limits](#hosting-calls-and-rate-limits)).

### Orphaned rounds

A nested run whose parent exits stops as `ORPHANED` (exit `75`), without
turning the lost parent into evaluator `STUCK`.

### Exit code 79 is terminal

Structural refusal (`79`, `BLOCKED_TERMINAL`) propagates without retry.
Rounds and re-reviews stay sequential at constant session depth. Recursion,
width, and memory safeguards remain in force and are never raised to recover.

### No short timeouts

Steps add no per-step or default recipe timeouts. Tests, builds, model calls,
and CI waiting follow existing timeout policy. Execute tools noninteractively
and assess failures; GNU `timeout` and `setsid` are not mandatory interfaces.
Usage budgets remain separate from tool timeouts.

## Resumability

Reuse completed work only after validation. A fresh host repeats work whose provenance it cannot
establish. PR comments never restore workflow authority.

Staging, accepted evidence, and loop completion are distinct. Rust filesystem
handling creates private state and immutable snapshots. Unsafe historical
entries are rejected or quarantined. Hashes do not make historical or
same-user edits trustworthy. Every resume captures authorization afresh and
refreshes mutable facts before finalization.

State is kept in a per-run directory by default, so a later drive reuses round
history and budget usage only when every drive passes the same explicit
absolute `autodrive_state_dir`; see [state directory](#state-directory).

### State directory

The runner decides where auto-drive state lives and binds that path into the
captured authorization before any child starts.

| Invocation | State directory |
| --- | --- |
| `-c autodrive_state_dir=/abs/path` | That path, unchanged; validated and bound, created only if it does not exist. |
| Omitted, normal run | `<durable run folder>/autodrive-state`, inside the run folder admitted by [recipe recovery](recipe-recovery.md#private-storage-and-publication). |
| Omitted, `autodrive_dry_run=true` | `autodrive-state` inside the private launch bundle; removed with the bundle after the report is produced. |

A supplied value that is empty, relative, contains a `..` component or
contains a control character stops the run before the runner probe or any
child starts, with the message `autodrive_state_dir must be an absolute path`.
When `auto-drive-light` is run directly, the recipe repeats these checks with
its own messages. An existing directory that is
a symbolic link, not a directory, owned by another user, or accessible to
group or others is refused; it is never re-permissioned to make it pass. The
runner creates the directory `0700` and evidence files `0600`. The durable run
folder must be owned by the current user and not writable by group or others.

The runner binds the absolute path it passes to the evidence store, before any
canonicalisation, and gives the child the same string. When the run ends, the
report opens the store only from that bound path. It compares the child's
`autodrive_state_dir` and the reducer's `state_dir` with the bound path
lexically: nothing is joined onto the working directory, canonicalised or
resolved through links. An empty string counts as absent. Any other value,
including a relative path, a sibling directory, a different absolute path or a
non-string JSON value, gives readiness `incomplete`, action `not_requested`
and a fixed `failure_cause` naming the state directory mismatch.

Because the default directory is per run, round history and budget usage are
shared across drives only when each drive passes the same explicit absolute
`autodrive_state_dir`. Records stay bound to the `run_id` that produced them,
so a shared directory cannot replay an earlier run's finalisation
observations or completion receipt into a merge-ready result.

## Trusted execution and portability

The launch bundle pins recipes, criteria, templates, evaluator, and helpers
independently of merge enforcement. Selection never falls back to PR-authored
instructions. Rust helpers validate and snapshot records, manage state, check
local Git relationships, and reduce outcomes. They do not contact hosting
services, judge semantic readiness, or merge.

Before any `git` or `gh` call, the merge-round and
merge-evidence steps clear the `GIT_*` repository selectors and the
outer run's `AMPLIHACK_*` names listed in
[`git-repo-env.sh`](git-repo-env-helper.md) (#2033). The merge round
finds its tools under `$AMPLIHACK_HOME`, `$REPO_PATH`, `~/.copilot`,
then `~/.amplihack`, and never searches the git toplevel, which an
inherited `GIT_WORK_TREE` could redirect. Merge-round QA evidence
records the cleared names, never values, in `git_env_cleared` and
`run_env_cleared`.

The implementation retires the shell gate and its enforcement dependencies.
The execution path does not require `/proc` process identity,
`autodrive_merge_gate.sh`, gate-resolved `gh`, a shim on `PATH`,
gate-specific `BASH_ENV`, or GNU-only `chmod MODE -- PATH`. Runner,
launcher, hooks, installation, and shared consumers follow this contract.
Cleanup removes only exact recognized retired entries and preserves unrelated
shell settings. Unrelated `MergeGate` ownership coordination is preserved.

Prompts, authorization records, and snapshots are workflow safeguards, not an
OS security boundary against an agent holding credentials. Repository
protections and minimum necessary credentials remain the service boundary.

## Target compatibility matrix

These six cells are acceptance targets for unchanged recipe YAML. No execution
evidence accompanies this specification, so none is claimed as verified.

| Host | Claude | Codex | Copilot |
| --- | --- | --- | --- |
| Linux | Target; untested | Target; untested | Target; untested |
| macOS | Target; untested | Target; untested | Target; untested |

Record actual execution evidence and tested revisions in PR descriptions or CI
logs. If a host or runtime is unavailable, mark that cell `unavailable` with
the reason; absence of execution evidence means `untested`, not an assumed
pass or assumed availability. Controlled tests on one cell cannot validate
another. Gate retirement and portable helper design alone do not establish
runtime compatibility.

## Related documentation

- [Verify a PR without merging](../howto/verify-pr-with-auto-drive.md)
- [Evidence and outcomes](auto-drive-evidence.md)
- [QA selection and configuration](auto-drive-qa.md)
- [Auto-drive-light](auto-drive-light.md)
- [Merge-ready authority](../../amplifier-bundle/skills/merge-ready/SKILL.md)
- [Framework source selection](framework-source-selection.md)
- [Crusty completion contract](autodrive-crusty-completion.md)
- [Git repository environment helper](git-repo-env-helper.md)
- [Expected checks at the head](autodrive-expected-checks.md)
