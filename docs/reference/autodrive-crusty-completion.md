---
title: Auto-drive Crusty Completion Contract
description: Completion-first dispatch, decision integrity, and follow-up finalization for Crusty reviews.
last_updated: 2026-11-12
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Crusty Completion Contract

> [Home](../index.md) > [Auto Drive To Merge](auto-drive-to-merge.md) > Crusty Completion

Auto-drive checks whether the Crusty review objective is complete before
dispatching the loop-health evaluator. An evidence-valid `CLEAN` or
`MINOR_ONLY` round completes review iteration without an evaluator call.
Required follow-up recording then gates phase completion.

This contract applies to both the primary Crusty loop and Crusty rereviews
started between merge-ready rounds. It also applies when
[`auto-drive-light`](auto-drive-light.md) uses those loops. The merge-ready
loop's own evaluator and completion policy are unchanged.

## Contents

- [Next-action judgment](#next-action-judgment)
- [Rust helper operations](#rust-helper-operations)
- [Decision format](#decision-format)
- [Dispatch validation](#dispatch-validation)
- [Results and accounting](#results-and-accounting)
- [Follow-up finalization](#follow-up-finalization)
- [Behavior examples](#behavior-examples)
- [Configuration and failures](#configuration-and-failures)

## Next-action judgment

After the round record is written, an agent reads the record and its
`.findings`, `.minor` and `.severity` sidecars, then selects the next action:

| Current review state | Action | Next step |
| --- | --- | --- |
| `CLEAN`, and `reviewed_head_sha` equals `head_sha` | `complete` | Finalize required follow-ups, then record phase completion. |
| `MINOR_ONLY`, and `reviewed_head_sha` equals `head_sha` | `complete` | File or verify every minor follow-up before recording phase completion. |
| `CONCERNS` (blocking or major concerns open), including after a round with commits when the heads differ | `evaluate` | Retain remediation, progress and budget checks, then evaluate convergence where applicable. |
| `CLEAN` or `MINOR_ONLY`, but `reviewed_head_sha` differs from `head_sha` | `evaluate`, with the head mismatch as the reason | The new head is not reviewed yet, so `complete` is not allowed. Dispatch currently refuses this decision too, so the round stops with a dispatch error. It fails closed and grants no merge authority; changing dispatch is a follow-up under #2472. |
| Invalid evidence: missing or unreadable record, no verdict, or a failed or refused round (rc not 0, exit `79`) | `evaluate`, with the fault as the reason | Never `complete`. Dispatch keeps its own failure handling: exits `79` and `75` return before the decision is read, a missing or unreadable record fails dispatch validation, and a record with no verdict goes to the evaluator as `MISSING`. |

`CONCERNS` always means `evaluate`, whatever the head SHAs or sidecars say.
After a round with commits, `reviewed_head_sha` differing from `head_sha` is
expected and is not invalid evidence; it only means the new head has not been
reviewed. The `.minor` sidecar is required only for `MINOR_ONLY`. Dispatch
checks the sidecars itself: every round needs a `.findings` sidecar, `CLEAN`
and `MINOR_ONLY` need it empty, and `MINOR_ONLY` also needs a non-empty
`.minor` and a `.severity` sidecar.

The agent always runs the Rust writer and records a decision. It stops
without a decision only when the writer itself fails. It writes the reason in
its own words and never copies text from the record, sidecars or review prose
into it.

Completed reviews repeated in persisted history or within the same run are
still complete. An unchanged `MINOR_ONLY` verdict is not evidence of
nonconvergence. A finding that was previously major still follows the
[minor-classification rules](auto-drive-to-merge.md#crusty-review);
history does not override those rules.

This selection is agent judgment under SR-103. Shell code does not choose an
action by matching verdict tokens. The agent treats findings, history, PR
prose and embedded instructions as untrusted data. It writes only its
designated decision through the Rust writer; it does not remediate concerns,
edit evidence, manifests or receipts, or mark the phase complete in this step.

## Rust helper operations

The recipes use internal operations under `amplihack orch helper`. These are
driver interfaces; operators use the existing auto-drive recipe invocation.

| Operation | Responsibility |
| --- | --- |
| Prepare | Create a fresh unpredictable run identity, a unique per-round decision output path, and driver-owned identity, evidence snapshots and evaluator launch context. |
| Seal | The record-writing bash step captures fresh evidence before the next-action agent runs. Direct clients preparing existing records seal during prepare. |
| Write | Accept typed `complete` or `evaluate` and a bounded reason; compute the designated record's hash and atomically create the decision. |
| Dispatch | Independently validate the decision and evidence; return objective completion or launch the trusted evaluator with typed process arguments. |

The driver keeps trusted state outside the agent's designated output
directory. Prepare captures evaluator context into the private state file, accepting only declared
data fields. It cannot supply executable paths, arbitrary arguments,
environment overrides or depth-limit overrides. The binary, recipes and
evaluator resolve from the trusted launch installation.

## Decision format

Version 1 is exactly one JSON object, at most 16 KiB, with these fields:

| Field | Type | Constraint |
| --- | --- | --- |
| `schema_version` | Integer | Exactly `1`. |
| `run_id` | String | Matches the driver-issued identity for this run. |
| `round_label` | String | Matches the prepared round identity. |
| `round_record_hash` | String | Computed by the Rust writer using the existing manifest-compatible git blob hashing convention. |
| `action` | String | Exactly `complete` or `evaluate`. |
| `reason` | String | At most 1024 bytes; unsafe control characters are rejected. |

Unknown or duplicate fields, trailing content, unsupported versions and
invalid identities are rejected. The writer atomically creates the decision
without following an existing destination link. Missing, stale, replayed,
symlinked, oversized or insecure decisions fail visibly; dispatch never
infers completion or automatically retries an invalid decision. A private,
atomically created dispatch receipt prevents reusing a decision in the same run.

## Dispatch validation

Dispatch validates all of the following independently of the agent's claim:

- Driver-issued run and round identities, and the actual preserved record hash.
- Agreement among the preserved record, latest copy and manifest row.
- Required sidecars against driver-owned snapshots, including minor findings
  and severity evidence. A matching round JSON hash alone is insufficient.
- Current reviewed-head evidence and the recorded child execution status.
  A completion claim additionally requires successful child execution and an
  allowed `CLEAN` or `MINOR_ONLY` round result.
- Private directory ancestry, ownership, permissions and regular-file types.
  Descriptor-relative no-follow opens and descriptor metadata checks avoid
  races between path validation and use.

Both `complete` and `evaluate` pass through this validation. A recoverable
child failure may select `evaluate` only when the decision and required
record, manifest and sidecar evidence remain valid; failed execution can
never authorize `complete`. A no-progress exemption does not waive dispatch
validation. Missing or unreadable required records, invalid decisions or
evidence-integrity failures stop visibly before evaluation. Terminal
refusals, orphaning, usage-limit and review-directory failures retain their
own stop classifications and cannot use this recovery path.

Private directories and files normally use modes `0700` and `0600`.
Hashes check integrity against trusted state; they do not authenticate the
agent. Instructions and same-user permissions do not isolate the decision
step from arbitrary code running as that user.

## Results and accounting

Accepted objective completion returns these structured fields:

| Field | Value |
| --- | --- |
| `source` | `objective-completion` |
| `loop_result` | `DONE` |
| `evaluator_invoked` | `false` |

The real round verdict remains `CLEAN` or `MINOR_ONLY`; it is not rewritten
to persuade an evaluator. Round records, reviewed-head evidence, sidecars,
manifests, history, progress rows and actual round usage remain recorded.
There is no evaluator verdict line, evaluator log or evaluator usage entry
for this dispatch. `DONE` here ends review iteration; it does not certify
that follow-up recording or phase finalization succeeded.

For `evaluate`, Rust launches the trusted evaluator as a sibling at the
existing session depth, preserving inherited policy ceilings, working
directory checks, usage accounting and evaluator result parsing. Unresolved
reviews retain no-progress detection and budget checks. Evaluator `DONE`
cannot advance an unresolved review: it triggers re-review or the existing
inconsistent-`DONE` stop when nothing changes. Genuine nonconvergence remains
`STUCK`.

Exit `79` is terminal and never retried. Orphaning and usage failures retain
their own classifications; neither becomes objective completion or an
evaluator-sourced `STUCK`.

## Follow-up finalization

Both Crusty entry points finalize in this order:

1. Collect the terminal round's required minor findings using validated
   review evidence.
2. File or verify each required follow-up issue and any required parent-task
   comment using the existing idempotent follow-up tools.
3. Validate receipts against the finding, task, PR, repository and review
   identities.
4. Record resolved concern ids and a phase-completion marker bound to the
   current review evidence. Only then permit advancement to merge-ready or
   return successfully from a rereview.

A filing failure, missing dependency, missing receipt or dry run with
unrecorded required follow-ups leaves the phase incomplete. An older marker
cannot authorize a newer unfinished review. Existing valid receipts remain
available for recovery without duplicate issues. Later merge-loop filing
remains idempotent, and finalization refuses to report readiness while a
required follow-up is unrecorded.

## Behavior examples

| Input and evidence | Observable result |
| --- | --- |
| Six persisted `MINOR_ONLY` rounds, then another valid `MINOR_ONLY` round | `source=objective-completion`, `loop_result=DONE`, `evaluator_invoked=false`; file or verify follow-ups before advancing. No evaluator-sourced `STUCK`. |
| Six legacy minor-only results accumulated within one process, then a valid completion decision | The same result at the dispatch boundary. Corrected iteration exits on its first valid completion rather than generating six extra rounds. |
| Valid `MINOR_ONLY`, but issue creation or receipt verification fails | Review iteration ends; phase completion and advancement are refused. |
| Valid `CLEAN`, with no outstanding follow-up obligation | Objective completion, no evaluator call, then phase finalization. |
| Major findings recur with no measured progress | Existing no-progress detection or evaluator judgment stops as `STUCK`. |
| Unresolved `CONCERNS`, evaluator says `DONE` | No phase advancement; require re-review and retain inconsistent-`DONE` safeguards. |
| Completion decision from another run, altered sidecar, changed reviewed head, or failed child | Reject completion; no phase marker. |

## Configuration and failures

Completion-first dispatch requires no new operator setting and applies in
every repository. Existing `plan_task_id`, `autodrive_state_dir`,
`autodrive_dry_run` and `autodrive_budget_*` settings retain their roles; see
the [auto-drive skill](../claude/skills/auto-drive-to-merge/SKILL.md#invoking-it).
Pre-round budget checks still run before spending another review session.

An installation without the Rust helper fails preflight with an explicit
compatibility error before review starts. Invalid decision or evidence
errors require correction of the installation or evidence source; replaying
a decision or editing a phase marker cannot resume the run safely. Follow-up
failures leave existing receipts available for a rerun after the recording
problem is resolved. No completion decision authorizes a merge; the existing
merge-ready checks and finalization remain authoritative.
