---
title: Auto-drive Worktree Recovery
description: Planned recovery of missing PR-head worktrees in a private auto-drive repository, failure classification, and required evidence.
last_updated: 2026-11-18
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive worktree recovery [PLANNED - Implementation Pending]

This reference specifies intended behavior for issue #2802. The current recipes
resolve existing worktrees but lack an explicit missing-worktree recovery
sequence. Recovery, the test harness, and CI integration described here remain
planned until their implementation lands. Existing behavior is identified below.

For an observed OPEN PR, the planned recovery fetches a missing head, creates
a separate linked worktree in the run's own repository, and verifies it before continuing.
The detached launch checkout remains at its base commit with its contents
unchanged. An operator clone's worktree cannot substitute for a worktree
registered in the private run repository.

## Contents

- [Entry points](#entry-points)
- [Repository and PR identity](#repository-and-pr-identity)
- [Recovery sequence](#recovery-sequence)
- [Existing worktrees and branches](#existing-worktrees-and-branches)
- [Current results and historical observations](#current-results-and-historical-observations)
- [Configuration and authentication](#configuration-and-authentication)
- [Failure classifications](#failure-classifications)
- [Planned behavior examples](#planned-behavior-examples)
- [Planned investigation record](#planned-investigation-record)
- [Required behavioral and release evidence](#required-behavioral-and-release-evidence)
- [Related references](#related-references)

## Entry points

| Recipe | Planned recovery step |
| --- | --- |
| `autodrive-build` | `step-01-build-preflight` and `step-03-resolve-pr` |
| `auto-drive-light` | Existing-PR worktree resolution |
| `autodrive-merge-loop` | `step-01-merge-loop-preflight` |
| `autodrive-crusty-loop` | `step-01-crusty-loop-preflight`, before existing-worktree reconciliation |

Recovery will be an agent responsibility within these steps. Existing build
preflight uses the `fast` model tier; build step-03 uses `strong`. Tier selection alone is
not evidence that recovery succeeds or fails.

Recovery applies only after observing the PR as OPEN. MERGED, closed,
unreadable, and ambiguous PRs retain their existing outcomes without recovery
mutations. A complete search finding no PR retains the normal new-build path.

## Repository and PR identity

The repository is derived from captured run context, including
`AMPLIHACK_RUN_CHECKOUT` where available, and its canonical Git common
directory. For a private run this is the bare repository at
`<work root>/exec/<hash>/repository.git`. The working directory alone does not
establish repository ownership.

PR metadata supplies the target and source repositories, head branch, and full
observed head SHA. Explicit operator PR selection takes precedence over other
PR inputs and branch discovery. Recovery preserves each entry point's existing
repository restrictions, discovery rules, and light-drive binding. It does
not turn same-repository discovery into permission to select a fork PR.

The fetch source must supply that PR's head: an established source-repository
remote or a target-repository PR ref where supported. Fork heads are supported
where the entry point permits that PR identity; fetching the target
repository's branch of the same name is insufficient.

## Recovery sequence

The planned entry-point instructions must carry out this sequence:

1. Inspect worktrees registered in the captured private repository. Reuse an
   existing valid candidate through the established validation path. An
   incompatible registration or occupied destination is an explicit failure,
   not proof that no worktree exists.
2. For genuine absence, fetch the head from a source associated with the
   observed PR. Bind the remaining work to its observed full SHA; fetch
   success alone does not establish a checkout at that SHA.
3. Resolve the destination through the trusted `workflow_worktree_root.sh base`
   convention in private-run context. The result identifies placement;
   repository ownership still comes from the captured Git common directory.
   The bare repository's internal `worktrees/` metadata is not a checkout
   destination.
4. Create a separate linked worktree on the observed head branch at the
   observed head SHA. Preserve existing branches under the rules below.
   Never repurpose the detached launch checkout or overwrite a destination.
5. Verify canonical common-directory equality with the captured repository,
   linked registration there, branch identity, and exact observed SHA. Require
   success from `amplihack orch helper autodrive worktree` as well.
6. Publish the absolute verified path as `repo`, initialize private state
   through existing helpers, and continue through the entry point's normal
   downstream steps. A successful recovery replaces the earlier absence
   blocker; unrelated blockers still prevent progression.

The existing Worktree helper checks the supplied path against its expected
head and branch and refuses invalid candidates. Its interface remains
`amplihack orch helper autodrive worktree --repo PATH --head SHA --branch BRANCH`.
`SHA` is the full observed object ID; `PATH` is the linked worktree root.
On success it returns JSON containing `worktree`, `head_sha`, `branch`, and
`verified: true`. It does not create or repair worktrees. Success in an
operator clone does not prove ownership by the private repository.

## Existing worktrees and branches

Recovery adds no cleanliness requirement. Dirty contents and unpublished
commits remain intact. It never resets, force-replaces a branch, switches an
existing checkout's branch, removes registrations, or deletes a worktree to
make room.

For a missing worktree, an absent local branch is created at the observed SHA.
An unoccupied branch already at that SHA can be reused. A behind branch can
advance only through the existing non-destructive fast-forward semantics.
Ahead or diverged history and branches checked out elsewhere are preserved;
they cannot be forced into a newly recovered checkout at the observed SHA.

Crusty's existing-worktree reconciliation remains distinct:

| Existing worktree position | Crusty behavior |
| --- | --- |
| At the observed PR head | Validate and continue when the other preflight conditions permit. |
| Behind the observed head | Fetch and fast-forward without discarding changes, then verify again. |
| Ahead of the observed head | Review the local head and record both SHAs. A receipt for the local head does not prove readiness at a different remote head. |
| Diverged, or fast-forward fails | Report `worktree-diverged` with both SHAs and preserve the worktree. |

Exact observed-SHA matching applies to newly recovered worktrees. It does not
remove Crusty's established ahead-worktree review behavior.

## Current results and historical observations

Build step-03 already consumes the complete `build_preflight` report. Under
the planned contract it must recheck current state and recover a worktree that
preflight found missing. After successful
creation and verification, the earlier absence remains historical evidence in
`reason` or `attempts`; it no longer belongs in current `blockers`.

This does not clear an access, identity, private-state, or implementation
failure. Current verified worktree evidence also cannot establish readiness
from old review or CI evidence.

An existing OPEN PR still skips `step-02-default-workflow`. Recovery completes
the build phase's PR resolution and supplies the verified `build_result.repo`
to later phases; it does not require implementation to run again. Fresh-build
discovery, typed finalization handoffs, review-directory resolution, and
merge-loop base synchronization retain their existing contracts.

## Configuration and authentication

Planned recovery is automatic for eligible OPEN PRs and adds no context key or CLI
option. Existing settings retain their meaning:

| Input | Recovery use |
| --- | --- |
| Captured run context and `AMPLIHACK_RUN_CHECKOUT` | Identify the private run repository; an inherited path alone grants no ownership. |
| Work-root configuration | Controls placement through the trusted worktree-base helper. |
| Explicit PR input | Selects the authoritative PR before discovery. |
| `autodrive_state_dir` | Preferred private state directory outside the PR worktree; otherwise use the existing full PR-identity key. |

Git and hosting access use existing configured authentication. PR prose never
supplies fetch commands. Metadata, paths, refs, URLs, and diagnostics are
untrusted data and are passed as quoted arguments rather than evaluated shell
syntax. Recovery preserves TLS and SSH host verification and does not embed
credentials in remote URLs, prompts, or evidence.

## Failure classifications

These classifications describe the missing-worktree recovery path; they do
not introduce new output fields or redefine unrelated entry-point outcomes.

Crusty will report each recovery failure through its existing JSON fields:
`should_run="false"`, a nonempty `skip_reason`, the operation and cause in
`reason`, a named current failure in `blockers`, and attempted operations,
sanitized diagnostics and available exit statuses in `attempts`. Keep the
observed PR identity and head; leave `repo=""` until all worktree checks pass.
The planned prompt will extend the reason vocabulary below, without adding
fields or changing their types. Downstream conditions still gate on
`should_run`, not on reason tokens.

| Observation | Planned Crusty `skip_reason` and diagnostic |
| --- | --- |
| Head fetch fails | `no-worktree`; concrete fetch diagnostic and exit status. Sanitize credentials while retaining the failure cause. |
| Fetch succeeds but does not supply the observed SHA, or branch/SHA differs | `worktree-identity-failed`; expected and observed identity and the failed check. |
| Branch conflicts, incompatible registration, or occupied destination | `worktree-creation-failed`; conflict and destination diagnostic; preserve existing history and contents. |
| Worktree creation fails after successful fetch | `worktree-creation-failed`; retain the Git diagnostic and exit status. |
| Common directory differs from the captured private repository | `worktree-ownership-failed`; expected and observed canonical repository identity. |
| Linked registration is absent or the Worktree helper refuses the candidate | `worktree-validation-failed`; failed check, helper diagnostic and exit status where available. |
| Private state cannot be established | Existing `no-state`, with `state_dir=""`; retain any unrelated blockers. |

These operation-specific tokens are planned additions to the recipe's current
reason list. The shipped `no-worktree` reason still covers any missing or
unverifiable worktree. Existing divergence remains `worktree-diverged`; a
creation failure must not be presented as divergence without that evidence.
Build preflight has no `should_run` or `skip_reason`; it retains its PR facts
and reports these failures in `reason`, `blockers`, and `attempts`. Other
entry points use their existing fields and gate progression on their existing
conditions; this contract adds no Crusty-only fields to those outputs.

Only an actual fetch failure uses `no-worktree` in this path. Creation and
validation failures cannot be relabeled as fetch failures. If the PR moves,
the verified checkout must still match the observed full SHA; a mismatched
candidate is an identity failure rather than permission to silently change
the target.

## Planned behavior examples

These are acceptance cases for the intended recovery, not observed test results.

| Starting state | Result |
| --- | --- |
| OPEN PR; private bare repository has only a detached launch checkout | Fetch the observed head, add a separate linked worktree on its branch at that SHA, verify it, and continue with its absolute path. Launch HEAD and contents are unchanged. |
| Matching worktree exists only in an operator clone | Preserve that checkout and recover in the private repository. |
| Preflight recorded absence; step-03 recovers successfully | Retain the observation in history, remove only the resolved absence blocker, and pass the verified `repo` downstream. |
| Head fetch returns `fatal: couldn't find remote ref` | Stop with `no-worktree`, the concrete diagnostic, and the fetch exit status. |
| Destination already contains a dirty incompatible worktree | Preserve it and report the conflict explicitly; do not replace it. |
| PR is already MERGED | Retain the merged outcome without fetching or creating a recovery worktree. |

## Planned investigation record

The investigation must distinguish source-confirmed omissions from causal
hypotheses. Record execution evidence in issue #2802 or private runtime logs;
this section defines the required record, not a completed investigation.

| Classification | Finding or question | Source or evidence required |
| --- | --- | --- |
| Confirmed omission | Build preflight, build step-03, light resolution, merge preflight, and Crusty preflight lack an explicit fetch/create/verify sequence for a missing worktree. | Read the entry-point prompts in [autodrive-build](../../amplifier-bundle/recipes/autodrive-build.yaml), [auto-drive-light](../../amplifier-bundle/recipes/auto-drive-light.yaml), [autodrive-merge-loop](../../amplifier-bundle/recipes/autodrive-merge-loop.yaml), and [autodrive-crusty-loop](../../amplifier-bundle/recipes/autodrive-crusty-loop.yaml). Existing behind-head repair does not recover absence. |
| Confirmed omission | The issue-2802 Rust harness, Gadugi scenario/driver, and CI integration described below are absent. | Repository file inventory and [CI workflow](../../.github/workflows/ci.yml); their proposed paths below are deliverables, not evidence of execution. |
| Hypothesis | The `fast` tier causes recovery failures that `strong` would avoid. | Run the same real-agent fixture and recovery instructions with recorded adapter/model configuration and operation traces. Current tier labels alone establish no cause. |
| Hypothesis | An earlier absence blocker survives successful recovery, or a verdict is lost or misclassified downstream. | Trace the complete preflight and step-03 outputs, verified Git state, round record, decision writer result, and downstream consumer at the same head. A missing recovery instruction alone proves neither verdict loss nor a propagation defect. |

For each run, retain the source revision, entry point, adapter/model, observed
PR state and SHA, captured repository identity, attempted operations and exit
statuses, helper output, current blockers, and downstream outcome. Mark each
hypothesis supported, refuted, or unresolved with its evidence; unavailable
credentials or an unexecuted case leave it unresolved. Do not treat the
required tests below as results already obtained.

## Required behavioral and release evidence

**Planned:** The Rust harness, Gadugi scenario and driver below do not yet
exist, and CI has no issue-2802 integration. The following requirements define
the evidence implementation must supply; this reference claims no passing run.

The planned `crates/amplihack-cli/tests/issue_2802_worktree_recovery.rs` must use
isolated Git fixtures and a real supported agent adapter to execute shipped recovery
instructions. Required fixtures include a bare private clone, detached launch checkout,
known PR-head branch and SHA, operator clone, and a `gh` metadata stub that
reports the PR OPEN and rejects unexpected operations. Positive cases require
an actual fetch. The harness must not implement recovery itself or supply
predetermined agent success.

Assertions must inspect private-repository registration, branch and exact SHA,
existing-helper verification, downstream consumption of the verified `repo`,
historical-absence supersession, unchanged launch HEAD and tracked/index
state, preserved sentinels and dirty contents, and operator-decoy preservation.
Coverage must also include valid reuse, fetch errors, occupied destinations,
invalid-helper candidates, permitted fork heads, and non-OPEN immutability.
Recipe-text and prompt-phrase assertions are not behavioral evidence.

The planned release scenario is
`tests/gadugi/scenarios/issue-2802-worktree-recovery.yaml`, with the thin driver
`tests/gadugi/drivers/issue-2802-worktree-recovery.sh`. Planned CI wiring must
run it using the installed binary and pinned runner. The driver must reuse the
real-agent path; it must not fetch, create worktrees, or manufacture success.
Neither the scenario nor its driver may run on the development host.

Fixture homes, agent configuration, credentials, state, and logs must stay
beneath private temporary roots with mode `0700`. Tests must never touch the
real `~/.claude` or `~/.amplihack`. CI must provision adapter credentials only
in a safe execution context; untrusted PR code must not receive secrets.

Release evidence must record the Actions run URL, pushed head SHA, job, scenario
step, actual conclusion, and Git-state, verification, and progression results
in the PR or CI logs. Missing credentials or provider access leave validation
explicitly incomplete. Pending, skipped, inaccessible, or prerequisite-failing
runs do not count as passing evidence. They also do not establish that agentic
recovery is unreliable.

## Related references

- [Build preflight decision](autodrive-build-preflight.md) — build or reuse and report fields.
- [Recipe checkout isolation](recipe-checkout-isolation.md) — private repositories and worktree placement.
- [Auto-drive-light](auto-drive-light.md) — PR selection, binding, and escalation.
- [Crusty completion contract](autodrive-crusty-completion.md) — preflight and existing-worktree reconciliation.
- [Auto-drive to merge](auto-drive-to-merge.md) — downstream review, readiness, and finalization.
