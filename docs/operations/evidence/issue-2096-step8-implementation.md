---
title: Issue 2096 implementation candidate and delivery blockers
description: Working-tree implementation evidence; PR 2290 remains undelivered and unaccepted.
doc_type: reference
last_updated: 2026-11-10
---

# Issue #2096 implementation evidence

**Changes required. PR #2290 has not been delivered or merged.** This record
describes a working-tree candidate, not accepted production guarantees. The
[requirement and review ledger](../../reference/recipe-checkout-isolation.md#requirement-and-review-disposition-ledger)
continues to track outstanding acceptance requirements.

## Preservation and branch identity

The requested worktree is on `feat/issue-2096-recipe-runs-and-auto`, at PR head
`60ecbe573b60ff30f47273510e48a90341ffd217`. Initial staged and unstaged binary
patches, index bytes, untracked inventory, and an untracked archive were saved
before edits. Historical recording diagnostics, the Rust probe, and regression
fixtures staged for deletion were restored on disk. They still appear as staged
deletions plus restored untracked paths because this environment cannot update
the shared index. Their evidence has not been superseded or discarded.

The observed current main is
`bd0df1988f9e9a7a354eb62ef49efb10f4ff8b57`; the branch and main each have three
commits after merge base `def287b8222c7a1df014e9397cf961d6536d8185`.
**Main was not incorporated.** Ordinary `git fetch origin main` fails opening
the worktree's `FETCH_HEAD`, and ordinary staging fails creating `index.lock`:
the shared Git metadata under `/home/ryan/src/amplihack-rs/.git` is read-only.
No implementation commit or PR push succeeded. No rebase, force push, hook
bypass, or merge to main was attempted.

## Implemented candidate

- Native preparation captures committed, staged, and working state separately,
  transports unreachable staged blobs, and provisions fresh private Git state
  before the runner starts. Initial runner cwd is private, not merely its `-C`
  argument. Inherited repository routing, configuration injection, executable
  extensions, and templates are scrubbed; context cannot restore those settings.
- Native ownership checks bind execution and runtime cleanup to filesystem
  identities and generation manifests. Independent nested runs get separate
  leases. Durable child worktrees retain their private metadata.
- Required Linux ptrace supervision follows runner descendants across
  fork/clone/exec. Recording is independent of Trace2 environment propagation.
  Per-attempt journals, scoped receipts, and final publication are fallible;
  failed collection or publication retains available evidence and fails the run.
  Nested scopes use kernel-verified tracer ancestry and explicit acknowledgements.
- Native review generations use exact PR-source commit identities, detached
  private repositories, and separate launch material. Verdict publication binds
  the generation and reviewed SHA. Existing mutation and failed-inspection
  distinctions remain. Each independently launched loop owns its cleanup chain,
  including nested re-review loops sharing an evidence directory.
- Executable provisioning, recording, validation, and release additions are
  Rust. Existing shell/YAML entry points delegate ownership operations to native
  commands; existing review/fix orchestration remains in place.

## Validation receipts

Final check results and complete logs are retained in the local implementation
handoff under `target/issue-2096-step8-evidence`, beside the complete source
bundle and original-state archives. Completed checks establish:

- Focused production-boundary contracts: 63 passed, zero failed, one opt-in
  real-hook replay ignored. This includes independent review cleanup and terminal
  reporting when opaque input mapping rejects launch.
- Recorder contracts: 20 passed on the final Rust candidate, including rejection
  of a native receipt from another run.
- Cleanup unit contracts: 16 passed.
- Git configuration/executable-injection policy regression and the context
  restoration rejection regression passed.
- Workspace Clippy with `--all-targets -- -D warnings`, formatting, shell
  syntax, and whitespace checks passed on the final Rust candidate. Native
  recipe validation passed for all four changed review/loop YAML files.
- The exact repository-pinned external runner revision
  `4d5187eaf95836a549e6e68725f8f853d0dc7034` was built and installed into an
  isolated temporary installation with its actual Cargo receipt. Coverage through
  that runner includes shell, a Rust agent fixture, absolute/cleared-environment
  Git, a Rust hook descendant, and independently owned nested execution.
  The host runner's mismatched receipt was not accepted or fabricated.

These are scoped behavioral receipts. A Rust agent fixture is not evidence
about a real agent CLI's automatic instruction/configuration discovery. The
older broad shell run from a partial immutable source snapshot had 871 passes
and eight failures. The final complete-source auto-drive shell suite has 878
passes and one failure: `1551-gate-merges-under-guard` receives the merge guard's
refusal. That guard is unchanged and was not bypassed. The cause of its refusal
has not been established here, so this remains a failed acceptance check.

Broader teardown unit execution produced five passes and eleven setup failures:
the fixture safety helper rejects temporary storage beneath framework ancestors,
then selects read-only `/var/tmp` in this environment. The safety helper was not
weakened. This does not establish teardown acceptance. All new fixtures use
temporary homes and explicit profile roots; no real Claude, amplihack profile,
or user profile data is a test fixture. No test sleeps or wall-clock assertion
windows were added.

The broader recipe execution unit module has 50 passes, 33 failures, and one
ignored test. All 33 failures occur during temporary-fixture construction with
read-only `/var/tmp`, before their execution assertions. No claim of a passing
full workspace test suite is made. A host Cargo wrapper also failed to acquire
its read-only cache slots; completed scoped checks invoke the actual Cargo
binary directly and retain the unsuccessful wrapper output as diagnostic data.

## Real-hook reproduction

The opt-in replay creates disposable linked worktrees, untracked caller input,
distinct staged/working private contents, isolated homes/profiles, the real
pre-commit framework, and a local bare remote. It retains caller index and HEAD
snapshots plus hook output and supplemental Trace2 data. Its baseline includes
tracked ignored inputs such as `Cargo.lock`; it does not replace the project
hooks with synthetic hooks.

A resumed replay of an earlier candidate completed an ordinary fixture commit,
`31e4f3f`, through all configured commit hooks. Caller index and HEAD comparisons
passed. This commit belongs only to the disposable replay branch, not PR #2290,
and does not validate later source edits. The ordinary local bare push reached
configured push hooks but was interrupted; full workspace acceptance was not
established. No successful local push receipt exists. The full real-hook finding
therefore remains open. Present fixture evidence does not identify the historical
writer responsible for the original incident.

## Review dispositions and deviations

The issue and every available issue/PR discussion were read, including retained
Crusty, code, security, and philosophy findings. A prior readiness statement was
not used as closure. The historical reviews describe earlier PR heads; no fresh
independent review of this candidate is claimed.

| Finding | Candidate evidence and disposition |
| --- | --- |
| P1 missing recipe execution isolation | Native launch changes and focused contracts now exist. Complete required ignored/external dependency declarations and opaque-path semantics remain open. |
| P1 missing per-drive PR-head/trusted review provisioning | Native exact-commit generations, separate launch files, mutation guards, binding, and independent cleanup now exist. Actual agent discovery and full remote PR/drive acceptance remain open. |
| P1 missing mandatory recording | Required native Linux runner-descendant observation and failure tests now exist. Preparation/release observation, renamed Git, unrelated writers, and arbitrary direct index writes are outside demonstrated coverage. |
| P1 incomplete real-hook replay | Ordinary fixture commit hooks and caller comparisons passed on an earlier snapshot; full local push and current-candidate hook acceptance remain open. Historical attribution remains unproven. |
| P2 missing boundary/concurrency/lifecycle evidence | Temporary Rust fixtures, socket rendezvous, nested acknowledgement, failure injection, substitution, and durable-worktree contracts improve evidence. Full retry/cancellation/platform combinations remain open. |
| Security ownership and cleanup | Manifests and filesystem identities are checked. Descriptor-relative destructive operations and recovery/reclamation for retained durable metadata remain open. Private repositories are not OS sandboxes. |
| Philosophy and documentation | Modules expose narrow native boundaries with no new framework, schema service, or network API. Documentation keeps planned guarantees distinct from the candidate and retains diagnostic evidence. |
| Integration and delivery | Blocked by read-only shared Git metadata. Main incorporation, current commit/push hooks, PR commit and ordinary push remain undone. |

The design's source grouping is implemented through narrow internal module
inclusions at existing launch/command boundaries rather than a new generic
`RunScope` orchestration framework. Ownership lives with the native checkout and
generation leases. No claim of complete recording, tamper resistance, platform
acceptance, or successful delivery follows from these structural choices.
