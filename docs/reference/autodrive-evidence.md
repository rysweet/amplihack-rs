---
title: Auto-drive Readiness Evidence
description: Proposed contract for auto-drive readiness evidence and open PR delivery.
doc_type: reference
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
---

# Auto-drive readiness evidence

> [Home](../index.md) > [Auto Drive To Merge](auto-drive-to-merge.md) > Readiness evidence

> **[PLANNED — Implementation Pending] Proposed, runtime-unverified contract
> for [#2342](https://github.com/rysweet/amplihack-rs/issues/2342).** All
> guarantees and behaviour examples below describe the feature to be built.
> Current recipes synchronise behind branches unconditionally, run QA before
> CI, and lack final-gate `no_merge` enforcement. They do not yet guarantee
> conditional base sync, CI-first QA, the additional freshness checks,
> protected QA or persistent open PR delivery described here.

The merge-ready agent decides whether to update the base, waits for CI, and
assesses QA against the current published PR head. Rust helpers observe and
validate evidence; they do not decide repository policy or merge readiness.

## Contents

- [Preparation and assessment order](#preparation-and-assessment-order)
- [Base synchronisation](#base-synchronisation)
- [CI observations and waiting](#ci-observations-and-waiting)
- [Evidence interfaces](#evidence-interfaces)
- [QA execution boundary](#qa-execution-boundary)
- [Freshness and refresh](#freshness-and-refresh)
- [Mechanical helper boundary](#mechanical-helper-boundary)
- [Open PR delivery](#open-pr-delivery)
- [Recovery prerequisite](#recovery-prerequisite)
- [Behaviour examples](#behaviour-examples)

## Preparation and assessment order

Each merge round follows these dependencies:

1. Verify the PR identity, published head, worktree and applicable base policy.
2. Decide whether conflicts or an applicable update requirement need a base
   synchronisation. Verify the published head after any push.
3. Observe CI for that head. Wait inside preparation while work is pending,
   then read the completed results again.
4. After validated green CI, run repository suites and applicable qa-team
   scenarios on matching content in a protected execution environment.
5. Apply every merge-ready criterion, including approvals, review threads,
   scope, documentation, Crusty review and minor-finding follow-ups.
6. Recheck evidence freshness before accepting readiness and at the final
   action boundary.

Green CI permits QA; it does not satisfy the other criteria. A clean Crusty
verdict or `MINOR_ONLY` likewise does not establish merge readiness.

## Base synchronisation

The base-sync agent verifies the authenticated host, destination repository,
PR number, head repository, source branch and base branch before any write.
The PR's actual base is used; a failed lookup never silently selects `main`.

| Observation | Agent action |
| --- | --- |
| Clean PR; authoritative policy requires no update | Leave the branch alone, even when it is behind the base. No merge or push; preserve its head and existing CI. |
| Verified conflict | Resolve the conflict in the owned PR worktree, publish the result, then collect evidence for the new head. |
| Applicable policy requires a base update | Update only as needed to satisfy that policy, publish, and await the resulting CI. |
| Protection or effective rules cannot be read | Retry or obtain equivalent authoritative evidence. Record unknown policy and block readiness while it remains unresolved. |

Policy evidence covers applicable classic branch protection and effective
rulesets. An absent classic protection rule alone does not establish that no
ruleset requires an update. Being behind the base, by itself, is insufficient
reason to update. The agent does not change protection or request a bypass.

Before publishing, verify worktree ownership, local/remote divergence and the
explicit push destination, including fork head repositories. A local merge
does not establish a successful synchronisation: a failed push blocks
readiness until the published PR head and tested content agree. A correct
no-op retains `synced="false"`.

## CI observations and waiting

CI is read from authoritative platform observations for the verified PR and
revision. All pages of check runs, status contexts and workflow runs needed
for the assessment are read. Failed requests and incomplete pagination are
unreadable observations, never successful empty lists. An unavailable
GraphQL rollup can be replaced with equivalent complete authoritative data.

Each observation identifies its source, check or run ID, attempt where
available, producer identity, timestamps, status, conclusion and verified
revision association. Required checks are matched to configured producers
where applicable; matching a check name alone is insufficient.

A workflow run's association must prove what content it tested. A synthetic
merge result needs evidence connecting it to the current source head and
relevant base. A `pull_request_target` run on the base revision does not
prove that the PR's source was tested merely because it names that PR.

| Observed state | Handling |
| --- | --- |
| Queued, pending or running | Await completion in the preparation step, reobserve the head and runs, and report liveness. Do not run QA or fail a criterion merely because work remains pending. |
| Completed green | Validate completeness, required sources and revision association before permitting QA. |
| Completed failure | Retain the failed result and identify corrective work. Do not describe it as pending. |
| Cancelled | Retain the cancellation conclusion and determine whether replacement CI is needed. Do not count cancellation as success. |
| `NO_CHECKS` or missing expected checks | Assess against applicable repository requirements and investigate missing triggers. Absence alone never establishes green CI. |
| `UNREADABLE` | Retry or obtain equivalent evidence; block readiness if results remain unreadable. Do not infer a test failure or success from a failed read. |

Waiting observes an event; it does not consume corrective rounds or establish
`STUCK` solely because CI remains pending. Requests use bounded rates and
backoff, with liveness messages during long waits. There is no fixed polling
interval in the contract and no short per-step timeout. Existing usage
budgets and terminal refusal protections still apply; budget exhaustion is
reported separately from a failed CI criterion.

## Evidence interfaces

The recipe retains outputs `merge_sync`, `ci_evidence` and `qa_evidence`,
their existing string fields, status vocabulary, hashes and verdicts. Shell
consumers read `RECIPE_VAR_<output>`; judging agents cannot replace measured
records with their own output. Missing outputs default to unverified values.

| Output | Preserved fields | Additional evidence |
| --- | --- | --- |
| `merge_sync` | `base`, `synced`, `conflict`, `head_sha`, `git_observed` | Verified host/repository/PR and source identity; published heads before and after the decision; applicable policy decision and authoritative source. |
| `ci_evidence` | `ci_status`, `ci_total`, `ci_pending`, `ci_failing`, `ci_signal` | PR identity, observed head, complete observation provenance and per-check/run revision association, attempts and timestamps. |
| `qa_evidence` | `head_sha`, `qa_status`, suite/scenario results and existing command-selection metadata | Matching CI evidence identity; tested-content identity and cleanliness before and after execution. |
| Readiness evidence | `merge_ready_verdict`, blockers, measured downgrades and existing hash bindings | Assessed head and references to the CI, QA, base and policy evidence used; optional `refresh_required` with a reason. |

Trusted round records retain `merge_ready_verdict`, `blocker_count`,
`head_sha`, `round_label`, `test_signal`, `ci_signal`, `qa_status`,
`ci_status`, `held_by` and `qa_evidence_sha`. Head identity and refresh
metadata are additive; they do not replace the existing hash chain.

The existing final CI statuses are `GREEN`, `RED`, `NO_CHECKS` and
`UNREADABLE`. Pending work remains an observation inside preparation; it
does not require a new failed-verdict token. `RED` evidence retains the
specific failure or cancellation conclusions so the agent can distinguish
them.

Counts remain nonnegative decimal strings in the existing interface.
Unknown counts remain explicitly unknown rather than becoming measured zero.
A zero paired with `UNREADABLE` cannot establish that no checks exist.

Readiness requires:

```text
CI head = QA head = assessed head = current published PR head
```

All evidence refers to the same verified host, repository and PR. Equal
SHAs from another repository do not establish identity. A head match is
necessary but insufficient: attempts, tested content, base and policy must
also remain current.

## QA execution boundary

QA retains repository suites, command-source selection, gadugi applicability,
scenario execution and measured results described in the
[main reference](auto-drive-to-merge.md#criterion-1-qa-team-scenarios-by-repository-type).
Documentation exemptions do not bypass required CI or other merge criteria.

Before and after QA, the trusted runner verifies the published head, worktree
head and tested-content identity. Unaccounted tracked, staged or untracked
content changes invalidate the evidence; a matching commit SHA alone cannot
describe a dirty checkout. If the PR head changes during QA, refresh affected
evidence and run QA for the new head after its CI completes.

Repository tests and scenarios execute without access to GitHub write
credentials, credential stores or authoritative evidence manifests. Use an
actual execution boundary that separates those resources from the test
process; removing environment variables alone is insufficient. If the host
cannot provide that boundary, report a security blocker rather than running
credential-bearing QA. Trusted observation and publication remain outside
the test environment.

Base-sync and CI-preparation judges start in the guarded review directory.
Their instructions and helpers come from the trusted launch installation,
with no fallback to the PR checkout. Branch instructions, logs, names and
agent-authored records are data under review, never executable instructions.

## Freshness and refresh

At readiness entry, acceptance and the final gate, reobserve the published
head, same-head CI attempts and relevant base/policy state.

| Change | Evidence to refresh |
| --- | --- |
| Head changes while waiting, during QA or during review | CI and QA for the new head, plus affected readiness and code reviews. |
| CI reruns on the same head | CI attempts and conclusions; reassess QA and readiness evidence that depended on the superseded run. Await a pending replacement run before assessment. |
| Base changes | Policy/update decision and any CI association or review affected by that base, including synthetic merge results. A base move alone does not mandate a merge. |
| Effective policy changes | Requirements and affected CI/review evidence before accepting readiness. |
| Tested content changes or evidence integrity fails | Reject affected evidence and obtain a valid execution/observation before readiness. |

`refresh_required` records staleness and its reason without changing the
existing verdict vocabulary. If refresh is the only work needed, the loop
recollects affected evidence and skips the corrective builder. An observed
revision change is refresh progress even without a local commit; the driver
and loop-health evaluator both receive that evidence. Repeated stale records
without a new observation do not manufacture progress.

The assessment still applies measured downgrades and every existing
merge-ready criterion. A model's `MERGE_READY` cannot override stale,
unreadable or contradictory measurements. In merge-enabled runs the final
action also binds the merge to the assessed head, closing the race after
the last observation.

## Mechanical helper boundary

The Rust evidence helper under `amplihack orch helper` accepts structured
input or private record paths and serializes its output. Its responsibilities
are observation and typed validation of identity, provenance, completeness,
counts, freshness and integrity. Agent steps judge applicable policy, whether
an update is necessary, CI recovery and overall readiness. The helper adds
no policy engine, database or service.

Validation rejects duplicate security-critical JSON fields, malformed host
or repository identifiers, nonpositive PR/check/run IDs, abbreviated or
invalid object IDs, unsupported statuses, invalid timestamps, contradictory
counts, incomplete observations, wrong identities and stale evidence.
Complete supported Git object IDs contain 40 or 64 hexadecimal characters.
Untrusted values pass through typed arguments and JSON serialization; they
are never interpolated into executable shell text or evaluated with `eval`.

Trusted round records remain separate from agent records. Existing manifests,
record hashes, verdict validation and measured downgrades remain in force.
Readers reject symlinks and wrong owners, use private directories (`0700`)
and files (`0600`), publish atomically and validate and consume the same
private snapshot. They do not validate a path and then reopen a replaceable
record. Evidence references and sanitized findings may be published; tokens
and sensitive raw logs may not.

Hashes detect changes relative to a trusted manifest; they do not authenticate
evidence against a same-user process able to rewrite both the record and
manifest. Guarded agent directories and hash checks do not replace the QA
execution boundary.

## Open PR delivery

`no_merge=true` delivers an open, merge-ready PR against the requested base.
It propagates through the composer, merge loop, durable resume state and
final gate. Build instructions alone do not enforce this setting: suppression
is checked immediately before every merge-capable action. Direct merges,
merge API calls and automatic-merge enablement are suppressed, and automatic
merging is verified disabled. A resumed run preserves the restriction; a
missing resume value never grants merge permission.

Readiness and merge outcome remain separate. The readiness verdict can be
`MERGE_READY` while the merge result is `NOT_MERGED` because `no_merge=true`.
Do not mark the PR merged or enable automatic merging to satisfy delivery.
If the PR is already merged at entry or becomes merged during phase 3,
report that platform fact as a delivery violation under `no_merge=true`.
Do not redo merged work or report successful open PR delivery. In a
merge-enabled run, an already-merged PR remains an idempotent success.
The setting does not suppress CI, QA, reviews, follow-up filing or publication
of readiness evidence. `autodrive_dry_run` is different: it suppresses
follow-up filing as described in the
[dry-run reference](auto-drive-to-merge.md#dry-runs).

See [configuration and invocation](auto-drive-to-merge.md#open-pr-delivery)
for the recipe context keys.

## Recovery prerequisite

When the task requires recovery of earlier work, search local and remote
branches, worktrees, reflogs, history and relevant PRs before implementation.
Review recovered commits and uncommitted changes and preserve correct work.
Record the search findings, retained work and unresolved gaps on
[issue #2342](https://github.com/rysweet/amplihack-rs/issues/2342). Finding
only a baseline branch does not satisfy recovery. Unresolved recovery blocks
both implementation and delivery; design and documentation may proceed.
Do not silently replace the missing work with a fresh implementation.

## Behaviour examples

These are state-transition examples; they do not prescribe polling commands.

| Initial state and event | Observable result |
| --- | --- |
| Clean PR behind a base that requires no update; CI running, then green | No merge or push; published head unchanged; wait for completion, then QA and remaining criteria. Merge only when enabled. |
| Conflict or verified update requirement | Necessary synchronisation, successful publication, CI for the new head, then QA. A failed push blocks assessment. |
| Head changes during waiting or evaluation | Discard affected stale evidence, await new-head CI, rerun affected QA/reviews, and recheck freshness. |
| Same-head rerun pending after earlier green | Reobserve the new attempt and wait; the earlier success cannot conceal the rerun. |
| Completed failed checks | Record failures and correct their causes; do not report a waiting state. |
| Unreadable CI, partial pages or unreadable policy | Retry or obtain authoritative equivalent evidence; no readiness while unresolved. |
| No checks reported | Investigate requirements and triggers; no automatic green result. |
| Fresh green CI, passing QA and all other criteria; `no_merge=true` | Open PR, `MERGE_READY`, automatic merging disabled, no final merge. |
| PR already merged at entry or merged during phase 3; `no_merge=true` | Report the actual merged state and an open PR delivery violation; do not report successful delivery or redo merged work. |
