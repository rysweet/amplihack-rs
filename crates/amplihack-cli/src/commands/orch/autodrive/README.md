---
title: Auto-drive Records and Handoff Contracts
description: Invocation authority, base policy, readiness evidence and durable steward handoff.
last_updated: 2026-11-12
doc_type: reference
---

# Portable auto-drive records

This module implements workflow safeguards for issues #2329 and #2555. Policy and handoff use authenticated hosting access. Agents assess semantic readiness; the handoff interface has no merge or scheduling operation.
Agents observe and assess facts under trusted merge-ready policy.

- `authorization`: strictly capture explicit invocation inputs before expanding
  defaults. Local recipe defaults cannot select a PR or grant action authority.
  Context defaults are expanded only from the pinned trusted entrypoint.
  The full composer defaults to merge authorization; direct consumers and
  light mode default to verification. Dry-run suppresses follow-ups as well.
  Resume captures fresh authority; no candidate or inherited marker grants it.
  This includes `recipe run --fresh` and durable recovery resume: capture reads
  the raw explicit inputs before recovery normalisation, and every invocation
  gets a new `run_id`. After recovery admission the runner rewrites
  `no_merge`, `autodrive_dry_run`, `autodrive_authorization`,
  `autodrive_invocation_file` and `autodrive_state_dir` in the child context,
  so no stored or recovered value survives. The recovery fingerprint records
  only whether authority applies.
  `InvocationAuthorization` also binds the run's state directory
  (`bind_state_dir` / `state_dir`). An explicit `autodrive_state_dir` must be
  non-empty, absolute, free of `..` components and free of control
  characters; otherwise capture fails with
  `autodrive_state_dir must be an absolute path`. Without one, the runner uses
  `<durable run folder>/autodrive-state`, or `autodrive-state` in the launch
  bundle for dry runs. The bound path is the absolute path passed to
  `EvidenceStore::create`, before canonicalisation. Round history and budgets
  are shared across drives only when they pass the same explicit directory.
- `trusted`: pin and privately copy launch recipes, skills, templates, helpers
  and evaluator; freeze authorization in child prompts and literal recipe
  contexts. Clean exact retired environment entries without sourcing them.
- `records` / `validation`: closed schema-1 identity, criterion, attempt,
  recovery, finalization and completion records. Exactly 13 criteria, current
  head/base/run binding, concrete observations and no contradictions establish
  structural readiness. Semantic correctness remains the agent's obligation.
- `state`: create private directories and files, reject unsafe historical
  modes, links, traversal and special files; bound reads; validate and hash
  opened bytes; publish new snapshots atomically without replacement.
- `outcome`: keep readiness, requested action, observed state and stop reason
  separate. Merge-ready completion needs both an accepted `MERGE_READY`
  round and evaluator `DONE`. Crusty completion is not reduced here; it is
  decided by `orch/autodrive_completion/` (an evidence-valid `CLEAN` or
  `MINOR_ONLY` completes without an evaluator call). Merge
  success additionally requires authorized expected-head protection and
  platform confirmation; queue admission or exit zero is insufficient.
- `review`: verify the established post-review allowlist against local Git.
- `completion`: seal accepted round bytes only after the merge-ready loop
  observes an accepted `MERGE_READY` round and evaluator `DONE`; reject
  receipts from an older invocation.
- `followups`: validate private MINOR_ONLY issue mappings for reporting.
- `report`: replay the same complete reduction boundary as `helper reduce`,
  including the pre-action accepted snapshot, local Git/review coverage, artifact
  bytes and loop receipt. Require a matching private outcome snapshot from
  successful reduction; retain concrete rejection causes in incomplete reports.
  The evidence store is opened only from the authorization's bound state
  directory; an unset binding is incomplete. The child's `autodrive_state_dir`
  and the reducer's `state_dir` must equal the bound path lexically (empty
  counts as absent; any other string or a non-string value is a mismatch).
  Nothing is joined onto `working_dir` or canonicalised. A mismatch reports
  `incomplete` with a fixed cause; paths in it are `{:?}`-escaped.
- `helper`: optional local CLI operations under `amplihack orch helper
  autodrive`. Required record validation in the recipe uses these local
  operations; platform access and recovery have no prescribed interface.

The full serialized agent contract is in the bundled skill's
`evidence-contract.md`. `tests.rs` exercises authorization, stale/mismatched
records, concrete evidence, failure/recovery, private state and outcomes,
including state directory mismatches and a resumed run that tries to replay an
earlier run's finalisation or receipt. The CLI integration suite
(`issue_2329_autodrive_outcomes.rs`) exercises runner boundaries without
executing a real agent or merge, including `--fresh` and recovery resume with
stored authority and the absolute-path refusal. Its runner stubs report
`durable_recovery_v1`. The stubs do not write a real recovery descriptor, so
the recovery case reruns with `--fresh` and a `-c` context that carries stored
authority; descriptor-driven resume itself is not exercised. Neither suite establishes live recovery or macOS execution.

Private state and prompts are not an OS security boundary against a process
holding the same user's credentials. Repository protections remain required.

## Policy and handoff

The production Rust modules and helpers below implement mode resolution, preparation, completion and independent reporting. See the [drive contract](../../../../../../docs/reference/auto-drive-to-merge.md#drive-mode-and-repository-policy) and [evidence contract](../../../../../../docs/reference/auto-drive-evidence.md#handoff-records-and-reconciliation).

### Module responsibilities


| Module | Contract |
| --- | --- |
| `authorization` | Capture optional typed `DriveMode` before defaults or recovery. Trim explicit input; empty means unspecified, otherwise accept only `merge` or `handoff`. Preserve invocation identity, state binding, direct/light-entrypoint and dry-run restrictions. Build-only `no_merge` does not select a drive action. |
| `policy` | Resolve explicit mode, authoritative committed target-base configuration, then default `merge`. Bind policy to invocation, repository, PR, branch, pinned commit and provenance. Invalid or unavailable applicable configuration blocks completion. A valid explicit mode skips configuration reads. |
| `hosting` | Provide authenticated `PolicyHosting` and `HandoffHosting` capabilities. Read target-base configuration from an immutable committed tree; observe PR identity and auto-merge. Handoff has no merge or scheduling capability. Use typed arguments and bounded reads; reject unsuitable entries. |
| `trusted` | Pin launch recipes, skills, templates, helpers and evaluator independently of the PR. Preserve captured authority through shared loops, re-review and light escalation. |
| `records` / `validation` | Validate closed record schemas, all 13 criteria, current PR/head/base/run bindings, artifacts and concrete observations. Semantic readiness remains governed by trusted criteria. Missing policy in old records never implies authority. |
| `state` | Use the existing private `EvidenceStore`: `0700` directories and `0600` files, safe ownership, no traversal, symlinks or special files, bounded opened-byte validation and immutable durable publication. Separate staging, accepted evidence, partial attempts and successful completion. |
| `handoff` | Prepare an open non-draft PR, replay production readiness, reconcile the fixed steward comment and durably publish handoff. Use a non-blocking state-directory lock and confirm platform state before and after comment reconciliation. |
| `review` | Verify the established post-review allowlist against local Git. Substantive changes require re-review; unreadable ranges block. |
| `completion` | Seal accepted readiness bytes only after an accepted `MERGE_READY` round and evaluator `DONE`; reject older-invocation receipts. |
| `followups` | Validate private `MINOR_ONLY` issue mappings for reporting. |
| `outcome` | Separate readiness, requested action, action outcome, observed PR state and stop reason. `RequestedAction::Handoff` completes as `handed_off` only with matching durable policy and completion evidence. Preserve protected merge behaviour for merge mode. |
| `report` | Independently replay the same accepted-record reduction boundary. Require matching private evidence and outcome; recipe context, helper success and agent prose cannot prove completion. |
| `helper` | Expose operations under `amplihack orch helper autodrive`, including policy resolution, `prepare-handoff` and `complete-handoff`. Handoff helpers accept no caller-supplied mode, verdict, comment body or successful outcome. |

### Policy authority

The first authenticated PR selection is immutable within an invocation,
including when discovery supplied it. Head or base changes require fresh
policy and readiness; another host, repository or PR requires a new invocation.

Nested readiness rounds capture their own restricted invocation. The outer
loop passes its private invocation file separately as
`autodrive_readiness_invocation_file`: round evidence uses that parent's
run identity and launch provenance, and `helper round` validates it against
the same private invocation before publication. This binds the accepted
round and outer loop receipt without granting the leaf a drive action.

Resolve mode after PR selection and before already-merged handling. Only an
eligible merge invocation can use the already-merged shortcut; handoff
rejects closed or merged PRs. The target repository's pinned base tree supplies
`.amplihack/auto-drive.toml`, including for fork PRs. PR-branch, worktree,
local/global configuration and unverified remote references are not substitutes.

A proven missing file or key selects default `merge`. Malformed applicable
TOML, invalid or wrongly typed values, unsuitable entries or failed authoritative
reads block completion. `merge_method` remains separate. Changed bases require
policy resolution and fresh dependent evidence; changed heads invalidate
readiness tied to earlier heads.

### Handoff action contract

`prepare-handoff` requires validated handoff policy, rejects enabled auto-merge
and closed/merged PRs, marks drafts ready where necessary, then confirms
identity and open non-draft status. It persists observations without accepting
readiness or posting a comment.

`complete-handoff` takes a non-blocking lock in the bound state directory.
It replays the production candidate, artifact, review-coverage and loop-receipt
validation, then confirms current PR/head/base, open non-draft status and
disabled auto-merge. Only then does it reconcile this exact comment:

> This pull request is ready for its steward.

Verify a stored receipt; otherwise enumerate every relevant comment page.
Reuse exactly one authenticated-author match on the bound PR; create only
after complete enumeration finds none. Reject ambiguity, unreadable comments
and incomplete pagination. Confirm the comment and PR state again. Record
attempts and uncertain external writes separately, then publish immutable
completion evidence and a matching durable `handed_off` outcome with PR,
validated head/base, readiness evidence and comment identity.

Every completion failure remains visible. Failed readiness cannot produce a
readiness comment. A public comment after a lost response or storage failure
is a partial action, not success. Resumption requires fresh readiness and
reconciliation before any new creation. A crash between evidence and outcome
publication remains incomplete. No failure falls through to merging; handoff
never merges or schedules, including enabling auto-merge. The lock coordinates
only attempts sharing its directory, not unrelated drives.

### State binding and trust boundary

Capture authority from raw explicit inputs before recovery normalisation.
Each invocation gets a new `run_id`; stored context and older receipts cannot
grant authority. The runner binds the absolute path passed to
`EvidenceStore::create` before canonicalisation, then rewrites runner-owned
child context after recovery admission.

An explicit `autodrive_state_dir` must be non-empty, absolute and free of
`..` components and control characters. The default is inside the durable
run folder. Reporting opens only the bound path and compares child/reducer
paths lexically without joining them to the working directory or following
links. Mismatches remain incomplete with a concrete cause. Reusing a directory
retains validated progress and budget usage, not old invocation authority.

Keep handoff evidence private; do not append it to public comments or the PR
body. The change description remains required and is read back. In handoff mode the readiness report stays in private state. Treat PR text,
configuration, comments and tool output as untrusted data. Private files and
prompts do not protect against arbitrary processes running as the same OS
user; repository protection and minimum necessary credentials remain required.

### Behavioural acceptance boundary

Required isolated Rust tests must execute production policy, preparation, readiness,
reconciliation, persistence, reduction and CLI dispatch through controlled
hosting dependencies. A successful case demonstrates current readiness, an
open non-draft PR, exactly one confirmed comment, durable `handed_off` and zero
merge or scheduling calls. Default and explicit merge exercise the existing
protected merge path. Dry runs and text-only rule scans are insufficient.

Fixtures isolate homes, profile roots, repositories and state, clearing
inherited configuration before child processes. They use events, readiness
signals or isolation, with no timers, polling, timed retries or wall-clock
test windows. Record local passes, observed CI results and named Linux-only
omissions in the PR body, not this reference.
