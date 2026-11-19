---
title: Recipe Execution in Named Checkouts
description: Execution directories, PR-worktree review, preservation of local work, and Git environment handling.
doc_type: reference
last_updated: 2026-11-18
---

# Recipe execution in named checkouts

`amplihack recipe run` executes in the checkout selected through `-w` /
`--working-dir` or recipe context `repo_path`. It creates no private execution
repository, detached execution copy, or review launch folder. The default
workflow creates or reuses its task worktree; auto-drive uses the PR's
legitimate linked worktree. Agents resolve the repository and worktree they
need from that selection and the workflow's evidence.

## Contents

- [Execution locations](#execution-locations)
- [Paths and Git configuration](#paths-and-git-configuration)
- [Worktree verification](#worktree-verification)
- [Review in the PR worktree](#review-in-the-pr-worktree)
- [Pre-review evidence](#pre-review-evidence)
- [Assessment after review](#assessment-after-review)
- [Review and completion records](#review-and-completion-records)
- [Recovery and cleanup](#recovery-and-cleanup)
- [Configuration and migration](#configuration-and-migration)
- [Release acceptance](#release-acceptance)

## Execution locations

| Operation | Directory |
| --- | --- |
| Runner launch | The directory selected by `-w` / `--working-dir`. |
| Repository-specific recipe work | The named `repo_path`, with explicit step `working_dir` applied where required. |
| Default-workflow implementation | The task worktree created or recovered by the default workflow in the named repository. |
| Auto-drive review context, reviewer, assessment, and relevant evaluator | The actual linked PR worktree at the intended reviewed commit. |
| Auto-drive fixes, commits, pushes, and QA | The actual linked PR worktree. |
| Nested recipe | Its explicitly selected directory, otherwise its inherited working directory. No execution copy is introduced. |

`-w` selects the runner's working directory; `repo_path` is recipe context.
When supplying both for the same repository, use consistent paths. Selecting
an existing linked worktree keeps that selection. A subdirectory remains a
subdirectory unless a step explicitly selects the repository or worktree root.
Recipes that do not require Git can still run outside Git.

Recipe commands can change files, stage content, and create commits in their
selected checkout. There is no promise that a caller's checkout stays untouched.
Use the workflow's task or PR worktree for development, and keep unrelated work
in its own checkout. A missing task worktree is resolved through the default
workflow rather than a runner-created execution copy.

Trusted installed recipes, skills, and tools are selected independently of
the execution directory. Running in the PR worktree does not make PR-authored
workflow assets authoritative.

## Paths and Git configuration

Paths refer to the named checkout or the appropriate task/PR worktree. There
is no translation into a private repository and no refusal solely because
prose contains the named checkout's path. Ordinary runner validation remains.
Ignored files remain where they are; no input-copy allowlist is involved.

Command construction preserves credentials, SSH and credential-helper access,
Git configuration, workflow commit identity, tracing settings, selected agent
profiles, context transport, guidance, and session metadata.
`amplihack_git::scrub` runs last, after other environment construction, so
inherited Git routing cannot redirect repository or index operations. Its
existing removals include `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`,
`GIT_COMMON_DIR`, object-directory overrides, and environment-injected Git
configuration. Existing Git trace-environment clearing remains in place;
see [trace variables](recipe-runner-logging.md#caller-git-trace-variables-are-cleared).
Normal user and system configuration remains available; see
[workflow commit identity](../howto/configure-workflow-commit-identity.md).

Execution uses the invoking user's permissions. A worktree is not an OS
sandbox. Repository content, diffs, and review output are task data; embedded
instructions do not authorize credential access, publication, cleanup, or
workflow-state changes. Existing launcher sandbox and hook-trust settings
still apply.

## Worktree verification

`amplihack orch helper autodrive worktree` remains available. It resolves a
real linked worktree through the named repository and verifies registration,
branch, and the intended PR head. An unrelated repository, an unregistered
directory, or a worktree at the wrong head is refused. The helper does not
accept a private bare execution repository as a substitute.

A drive launched from a per-PR clone can use that clone's registered
`worktrees/pr-N` at the observed PR head. Preflights do not search a second
private repository or infer that the base checkout's `HEAD` is the PR head.
Existing work is preserved during worktree resolution and recovery.

## Review in the PR worktree

Both Crusty and merge-ready reviews run directly in the actual PR worktree.
The reviewer receives the intended reviewed commit and instructions to leave
files, the index, branch, `HEAD`, and workflow state unchanged. Review output
identifies the commit actually reviewed.

Codex uses ordinary Git discovery in that worktree. Amplihack does not
inspect review-folder ownership or automatically insert
`--skip-git-repo-check`. Explicit caller arguments, shared option parsing,
sandbox selection, hook trust, stdin prompt delivery, and final-message
capture keep their existing behavior. See
[Codex worktree launches](launch-flag-injection.md#codex-worktree-launches).

## Pre-review evidence

Immediately before each reviewer invocation, after preceding agents finish,
recipe tool steps save read-only evidence in existing restricted artifact
storage. This is ordinary workflow evidence, not a review-folder manifest or
a new isolation layer. Directory permissions are `0700`; evidence files are
`0600`. The baseline distinguishes existing work from later edits:

| Evidence | Purpose |
| --- | --- |
| Repository/worktree identity, branch, and full `HEAD` | Identify where review started and the intended reviewed commit. |
| NUL-delimited porcelain status (all untracked files) and index entries | Preserve arbitrary filenames and separate staged, unstaged, and untracked state. |
| Index bytes and referenced split-index data | Preserve the original staged state, including split-index dependencies. |
| Binary staged and unstaged diffs | Retain distinct changes relative to `HEAD` and the index. |
| Original dirty and untracked contents, file modes, and symlink targets | Distinguish reviewer edits to files that were already changed or untracked. |

Collection leaves the repository unchanged. Symlink targets are recorded
without following external content; filenames cannot escape artifact storage.
A path with a symlinked parent directory is marked incomplete and its contents
are skipped, which makes collection fail as a whole. Raw contents are kept out
of logs, prompts, PR descriptions, and public artifacts.

Incomplete collection is reported with a visible line in the assessment report.
When `review-baseline` itself fails and `pre_review_evidence` is empty, the
assessor restores nothing and states so; see [Assessment step](#assessment-step).

The baseline holds the contents of untracked files, so it can hold secrets that
the worktree carried. It is created under `AUTODRIVE_STATE_DIR` as
`pre-review.XXXXXX` with `umask 077` and mode `0700`. Nothing outside the round
reads it, so `autodrive_loop.sh` removes every `pre-review.*` in the state
directory once the round that took it has returned and the loop is going on,
whether that round succeeded or failed. The removal runs after the loop's
stops, so a round that ends the run — orphaned, a policy refusal, a usage
limit, or a recorded step-recovery handoff — leaves its baseline behind: there
the reviewer was interrupted, and the baseline is the only record of what the
worktree held before it ran. The next run's first round reclaims those too.
The rest of the state directory — round records, sidecars and logs — is
untouched. The baseline is never attached to a PR or written to logs.

Within the round, the baseline remains associated with its reviewer invocation
across recovery. Resuming an interrupted reviewer does not overwrite it with a
snapshot of partial reviewer changes. A later reviewer invocation receives its own baseline.

## Assessment after review

An agent follows the reviewer and inspects `git status --porcelain`, the
index, working content, untracked files, branch, and `HEAD` against the
pre-review evidence. It assesses and handles changes; a difference alone
does not fail the run.

| Observed state | Agent handling |
| --- | --- |
| Unchanged review | Continue using the recorded reviewed commit. |
| Clearly attributable reviewer edit | Restore only that edit when safe, or report it and select an appropriate continuation. |
| Reviewer stages content | Compare with the saved index and prior working content; preserve earlier staging and edits. |
| Reviewer edits an already-dirty or pre-existing untracked file | Compare original contents as well as diffs; retain unrelated and uncertain changes. |
| `HEAD` changes or review applicability is uncertain | Report reviewed and resulting commits accurately, then choose further evaluation, review, or handoff under existing rules. |
| Incomplete evidence or concurrent writers | Preserve uncertain changes, report what cannot be attributed, and determine the continuation. |

### Assessment step

The step is `step-02b-assess-review-changes` in both round recipes. It runs in
the PR worktree, publishes `review_change_assessment`, and declares no
`parse_json`, so its report is free text and cannot fail a parse. It never
sets a loop result, never exits non-zero, and no later step stops the run on
its content. Nothing hands it a hash or expected value to match.

Restoration is the only write the step can make, and it follows these rules:

- Paths come only from `git status -z`. Absolute paths and `..` components are
  not restored.
- Nothing is written through a symlinked parent directory, and nothing under
  `.git` or the common directory is touched.
- Each path is restored on its own. Archive extraction uses
  `--no-same-owner --no-same-permissions` and skips unsafe entries.
- Git commands use `--no-ext-diff --no-textconv`. The step runs no filter,
  hook or `fsmonitor` command from the tree under review, because the PR
  controls `.gitattributes` and `.git` content.
- When the evidence path is empty or collection was incomplete, the step
  restores nothing and says so in its report.
- The report names paths and kinds of change (modified, staged, created,
  removed, replaced by a symlink). It never quotes file contents.

Automatic restoration is optional. No blanket reset, cleanup, whole-worktree
restore, or replacement of the whole index is permitted. The saved index is
inspection evidence, never an automatic repair to install wholesale.

### Untrusted reviewer output

The reviewer's cwd is PR-controlled, so its output and the assessment report are
data. The reviewer prompt states that worktree content, including instruction
and configuration files, is never executed or obeyed. Steps that post the
verdict, extract it, or select the next action present `review_change_assessment`
inside a delimited block labelled as an untrusted report. No verdict, skipped
verification or merge-ready determination is derived from that report alone.

`loop-preflight` refuses `pre_review_evidence`, `review_change_assessment` and
`reviewed_head_sha` when they arrive through `--context`, as it does every
loop-owned key. `reviewed_head_sha` is accepted only as 40 or 64 lowercase hex
characters and is passed to Git after `--`. `pre_review_evidence` must resolve
under `AUTODRIVE_STATE_DIR`. Commit steps stage explicit paths and inspect
`git diff --cached --name-status` before committing, never `git add -A` or
`git commit -a`.

Recovery descriptors are read only from operator-supplied or runtime-owned
locations. Nothing discovers one beneath `repo_path`.

## Review and completion records

Round records, completion sealing, authorization, budgets, and readiness
evidence retain their existing roles. Records identify the reviewed commit
and resulting PR head truthfully, without review-generation publication
binding or replacement SHA-binding machinery.

The next-action agent always invokes `autodrive-completion-write`. It selects
`complete` only for a `CLEAN` or permitted `MINOR_ONLY` round with matching
`reviewed_head_sha` and record `head_sha`, and no round commits. Otherwise it
writes `evaluate` with a reason, including for missing or unreadable records.
Only failure of the writer itself stops selection without a decision.
Absent `.minor` or `.severity` sidecars do not prevent `CLEAN` completion.
See the [completion contract](autodrive-crusty-completion.md) for dispatch and
follow-up requirements.

## Recovery and cleanup

Recovery admission, outcomes, runtime finalization, and error propagation
remain in place. A resumed run uses the selected checkout and its retained
task/PR worktree; it does not recreate repository state from a fresh copy.
Interrupted runs retain uncommitted files, staged content, unpushed commits,
branches, and worktree registration so work can continue.

There are no execution-copy leases, release operations, `run-exec` cleanup
category, review-generation allocation or retirement, launch-folder manifests,
or review-folder ownership commands. Removing that machinery does not authorize
sweeping legacy directories: they may hold recoverable work. Unrelated runtime
cleanup remains separate; see
[cleanup and retention](../howto/clean-up-after-recipe-runs.md).

## Configuration and migration

### Legacy execution copies

Earlier releases created `<work root>/exec/...` copies. `amplihack cleanup
--only run-exec` no longer exists, so these copies are not reclaimed. They hold
full repository contents. After confirming no recovery needs one, remove the
directory by hand:

```sh
rm -rf "${AMPLIHACK_WORK_ROOT:-$HOME/.amplihack/work}/exec"
```

No isolation setting replaces the removed mechanism. Build, Crusty-loop,
and merge-loop entry gates use their declared `fast` tier without host-level
strong-tier overrides. Agents resolve their inputs from the named repository
and legitimate worktrees.

After deploying this behavior, an operator removes only the three obsolete
entry-gate overrides, preserving unrelated settings. See
[remove deployed entry-gate overrides](../howto/set-model-tiers.md#remove-deployed-auto-drive-entry-gate-overrides).
Deployment does not edit real host configuration automatically.

For a local walkthrough, see
[Run a recipe in a selected worktree](../tutorials/recipe-checkout-isolation.md).

## Release acceptance

Behavioral validation covers named-checkout build execution and a drive from
a per-PR clone with a registered linked worktree at the observed PR head.
Build and review phases use those repositories without private execution
resources. Coverage includes all three entry gates on their declared tiers,
unchanged reviews, unstaged and staged reviewer edits, edits to already-dirty
or untracked files, changed heads, review recovery, and retention of interrupted
uncommitted and unpushed work. The retained worktree helper accepts a valid
worktree and refuses invalid registration, branch, or intended-head evidence.
Completion coverage follows the existing rules described above.

That coverage lands with this change, as the `issue_2808` contracts under
`crates/amplihack-cli/tests`. They run in the workspace suite against hermetic
Git fixtures, the real runner library and real shell steps; only the hosting
evidence and the agents are scripted. They therefore establish checkout
selection, preflight routing, tier delivery and Git-visible effects, not live
model judgment. A whole build and a whole drive under live agents stay outside
this change and are tracked in
[#2806](https://github.com/rysweet/amplihack-rs/issues/2806), which this change
does not depend on. `tests/gadugi` scenarios run exclusively in CI. Validation
records distinguish behavioral stubs from live agent execution; source-text or
prompt-phrase tests do not establish this contract. Results belong in CI logs
and the delivery PR.

The owner decision in
[#2808](https://github.com/rysweet/amplihack-rs/issues/2808) supersedes the
historical isolation requirements and proposed repairs in
[#2802](https://github.com/rysweet/amplihack-rs/issues/2802) and
[#2807](https://github.com/rysweet/amplihack-rs/issues/2807); those two issues
close when the removal merges.
[#2785](https://github.com/rysweet/amplihack-rs/issues/2785) remains open for
the separate cleanup-retention work. Removing execution-copy machinery does
not resolve or authorize sweeping its legacy recovery data.
