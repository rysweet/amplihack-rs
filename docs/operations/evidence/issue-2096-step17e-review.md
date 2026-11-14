# Step 17e blocking review follow-up

Reviewed the comprehensive, security and philosophy findings on PR #2290,
comments 6773609639, 6773633276 and 6773658736, against head
`a2f92b847b8f423f45aec0cb0e4ea4ae0ba6117d`.

## Corrections

- Required command-log traversal now propagates missing storage, directory
  iteration and metadata errors with the failing path. Recursive errors cannot
  silently produce a partial evidence set. Added missing/invalid-storage and
  nested-record regression cases.
- The runner fixture checks every Git observation's exit status. Only
  `symbolic-ref -q HEAD` exit 1 is accepted for a detached HEAD. Required tracked
  input failures and unexpected optional-input read errors now stop the fixture;
  only absent optional inputs are skipped.
- Restored the pre-PR operational auto-drive reference, with an explicit link to
  the planned isolation contract. Proposed behavior no longer replaces current
  operating instructions.

## Validation and review disposition

Direct Rust compilation of the runner fixture passed with pinned Rust 1.97.0,
the Command Line Tools clang linker and SDK. Standalone execution of the exact
command traversal helper passed missing-storage, invalid-storage and nested-log
cases. Rustfmt checks and `git diff --check` passed for the corrections.
Disposable-repository runner checks passed for attached and detached HEAD,
absent optional inputs, missing required input, an optional input replaced by a
directory, and a failing Git executable. The failure cases exited unsuccessfully
with the expected observation or input error.
These targeted checks do not constitute CLI integration or security acceptance.

The prior step-16b Cargo receipt is now complete with exit 101, rather than
pending. No production integration pass is inferred from its compilation log.

Static re-review of the corrections resolves the fixture error suppression and
operational-documentation findings. The following blocking findings remain:

1. Production checkout preparation, caller input preservation, routing/path
   enforcement and ownership-validated lifecycle management are unimplemented.
2. Production per-drive provisioning at a recorded PR head, trusted reviewer
   configuration, generation-bound guard checks and publishing are unimplemented.
3. A supported supervised descendant recorder with fail-closed initialization,
   collection, persistence and finalization is unimplemented. Cooperative Trace2
   and wrappers do not satisfy that requirement. The retained macOS observer
   denial does not establish recording coverage.
4. The complete real-project-hook reproduction and caller HEAD/index evidence
   remain incomplete. Historical attribution remains unproven.
5. Actual agent/hook execution-boundary tests, synchronized concurrent/nested
   runs and drives, lifecycle recovery, and completed platform validation remain
   outstanding.

Overall code, security and philosophy disposition remains **changes required**.
PR #2290 must remain unmerged and issue #2096 open. The PR's `Closes #2096` and
"implemented behavior" claims remain inaccurate; this follow-up does not claim
delivery of the production feature or successful full reviews.

## Addendum, 12 Nov 2026

The text above records the review of head `a2f92b8` and is left as written.
The run on 12 Nov 2026 built on head `60ecbe573` and addressed the five
blocking findings as follows. Test results for this run are in the PR #2290
description and CI; whether issue #2096 closes is decided at the merge gate.

1. Preparation, input copying, path mapping and owned release: the batch
   staged-blob query takes object IDs only and rejects unexpected output;
   release deletes only manifest-owned paths.
2. Review generations at the recorded PR head: the guard script comments now
   name the three checks Rust makes (ownership manifest, identities, detached
   `HEAD` at the recorded PR-source commit).
3. Recording: dry runs use `GitRecord::dry_run()` and never start the
   observer; real runs fail closed. The observer is Linux-only.
4. Caller `HEAD` and index: `CallerGitState` and its #964 `core.bare` repair
   are removed, because no step runs in the caller's checkout.
5. Lifecycle: early failures finish the recovery admission as `Failure`;
   release runs in the order checkout, recording, admission, workspace, with
   every error kept; resumed runs prepare a fresh execution checkout at a
   path derived from the recovery identity, so replay can still reuse
   checkout-dependent steps (#2493). Test helpers fail on I/O errors and
   non-zero exits.

The current contract is in
[recipe checkout isolation](../../reference/recipe-checkout-isolation.md).
