---
title: Owned run directory cleanup
description: Ownership, root validation, and removal contracts for disposable recipe runtime directories.
doc_type: reference
last_updated: 2026-11-12
review_schedule: as-needed
---

# Owned run directory cleanup

The shared authority in `cleanup/owned_run_dir.rs` provides the implementation
described below. Treat the cleanup contract as
validated only when commit-specific validation and final review evidence meet
the [required evidence](#required-validation-evidence).
Linux cleanup requires descriptor-relative operations, no-replace claims,
mount IDs, and directory birth identities. Other platforms keep recipe
execution available and preserve runtime directories without deletion authority.
Linux filesystems that cannot supply the required identities or operations
also fail closed: initialization refuses to arm cleanup, or removal preserves
the directory and reports the missing guarantee.

Recipe runtime cleanup removes only a directory the runner created exclusively
and can still identify. Explicit run completion, `RuntimeGuard::Drop`, panic
unwinding, dry-run disposal, and stale `run-tmp` cleanup use the same removal
authority. Failure to establish permission to delete preserves the candidate.

For configuration and commands, see
[Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md#where-a-runs-temp-folder-goes).

## Contents

- [Roots and configuration](#roots-and-configuration)
- [Ownership established at creation](#ownership-established-at-creation)
- [Deletion authorization](#deletion-authorization)
- [Claiming and removal](#claiming-and-removal)
- [Lifecycle and stale cleanup](#lifecycle-and-stale-cleanup)
- [Internal API responsibilities](#internal-api-responsibilities)
- [Outcomes and retained directories](#outcomes-and-retained-directories)
- [Validation status](#validation-status)
- [Regression contract](#regression-contract)

## Roots and configuration

Root selection retains the expected parent independently of the deletion
candidate. Cleanup never derives that authority from the candidate's parent
or an ownership-record field.

| Layout | Disposable directory | Independently retained parent |
| --- | --- | --- |
| External | `<selected root>/ahr-<short id>` | Root selected from `AMPLIHACK_RUN_TMP_ROOT`, or the caller's suitable `TMPDIR`. |
| Default | `<work root>/runs/<run id>/runtime` | The run directory selected during run initialization. |
| Shortened | `<work root>/r/<short id>` | The selected work root's `r/` directory. |

`AMPLIHACK_RUN_TMP_ROOT` takes precedence over the caller's `TMPDIR`. An
unusable explicit root falls back to the default layout rather than trying
`TMPDIR`. The shortened layout applies when the default `runtime/tmp` path
would exceed 80 bytes. Root selection and path length restrictions
are described in [root configuration](../howto/clean-up-after-recipe-runs.md#rules-for-the-root).

The external prefix is `ahr-`; the dry-run prefix is `ahd-`. With a
12-hex-digit run suffix, `/ahr-<12 hex>/tmp` adds 21 bytes to the resolved
root. The 80-byte external `TMPDIR` limit therefore permits a 59-byte root
and rejects a 60-byte root. These names, limits, diagnostics, and boundary
tests must change together. The shortened layout still needs a sufficiently
short work root; selecting it alone does not guarantee socket compatibility.

The configured parent may itself be a temporary root. An eligible child can
be removed; that parent cannot. Root selection follows supported symlink
aliases and retains the resolved directory's identity. This does not allow
a symlink as the disposable directory itself.

External `run.json` reports record both `runtime_dir` and its separately
selected `runtime_root`. Stale cleanup requires that recorded root and the
matching creation records; legacy reports cannot grant deletion authority.

Steps receive `AMPLIHACK_WORKFLOW_RUNTIME_DIR` and `AMPLIHACK_RUNTIME_ROOT`
as the disposable directory, `AMPLIHACK_WORKFLOW_ARTIFACT_DIR` as its
`artifacts/` child, and `TMPDIR`, `TMP`, and `TEMP` as its `tmp/` child.
External runs pass the selected parent to nested runs through
`AMPLIHACK_RUN_TMP_ROOT`, so nested runs create siblings. These environment
values describe locations; none grants deletion permission.

`AMPLIHACK_CLEANUP_ON_EXIT=0` disables the end-of-run sweep. It does not
disable guarded cleanup of the completing run's own disposable directory.
There is no setting that bypasses ownership or protected-root checks.

## Ownership established at creation

Creation uses a single normal leaf component directly beneath the retained
root. Absolute names, separators, `.` and `..`, and reserved metadata or
claim names are rejected. Creation is exclusive: an existing directory,
file, or symlink is never adopted or marked for later deletion.

The authority captures the root and candidate identities and writes two
matching records: an internal creation marker and an external record in
private metadata storage. Each record binds the supported schema version,
run identifier, original leaf name, root identity, candidate identity, and
a nonce generated with operating-system-backed randomness. Ownership is
armed only after both records are complete and validated.

On Unix, metadata directories use mode `0700` and records use mode `0600`.
The authority checks their ownership and permissions without loosening
existing restrictions. It reads records through no-follow descriptors with
bounded, strict parsing. Each record must be a regular file with one hard
link; unreadable, symlinked, hard-linked, oversized, malformed, unsupported,
or inconsistent evidence fails closed.

A marker-shaped file alone proves nothing. Copying records to another
directory cannot authorize that directory because its identity does not
match. Neither `run.json`, a run name, `owned-paths`, nor a cleanup claim name
substitutes for creation evidence. Legacy unmarked directories remain intact.
Incomplete initialization is reported and preserved rather than made eligible
by a later cleanup attempt. Marker names and serialization are internal
details, not a user API.

## Deletion authorization

Every removal checks all of the following:

| Check | Required result |
| --- | --- |
| Creation evidence | Both records are valid, mutually consistent, and bound to the intended run directory. |
| Direct parent | Candidate is a direct child of the independently retained root. |
| Root identity | The root still identifies the retained directory. |
| Candidate identity | The opened directory still matches the creation records and retained identity. |
| Target type | Candidate is a real directory, never a symlink. |
| Protected roots | Candidate is neither the configured parent nor a protected directory, including aliases. |
| Filesystem support | Required no-follow, atomic claim, descriptor-relative removal, and mount identity guarantees are available. |

Protected directories include the active `TMPDIR`, `std::env::temp_dir()`,
literal `/tmp`, and `$HOME`. The authority retains initial protected-root
observations and refreshes environment-derived observations before removal.
Changing an environment variable cannot remove an earlier protection. A candidate
containing a protected directory is also refused; verified children beneath a
protected root remain eligible.
Unresolvable active observations or inconsistent identities preserve the
candidate.

Comparisons use resolved paths and filesystem identities where needed.
Relative components, equivalent spellings, symlink aliases, and other
filesystem aliases cannot bypass the checks. A pathname string comparison
alone is insufficient. Root or candidate replacement, or a resolution
failure, preserves the candidate. Validation continues to apply when removal
occurs, rather than ending after a preliminary canonicalization.

## Claiming and removal

The authority serializes cooperating cleanup claims, opens the candidate
relative to its retained root, and validates authorization. It then claims
the directory with an atomic rename that cannot overwrite an existing
entry, verifies the claimed identity, and revalidates before traversal.
A claim name carries no independent authority.

Removal traverses through directory descriptors without following symlinks
or crossing mount boundaries. Symlink entries inside the authorized tree
can be unlinked; their destinations are untouched. Mount identity checks
also distinguish same-device bind mounts: device numbers alone are not
sufficient. Unsupported platforms or missing filesystem guarantees preserve
candidates instead of falling back to unrestricted pathname recursion.

A failed claim is restored only when restoration cannot overwrite another
entry. Otherwise the preserved claim location is reported. Interrupted cleanup
can leave a retained claim. Its name is derived from the nonce in both creation
records, binding it to the original leaf. `OwnedRunDir::load` accepts the original
path and separately selected parent, and can recover only that matching claim
when the original entry is absent. An occupied original entry prevents recovery.
Both records, parent and candidate identities, protected roots, and process use
are rechecked. A claim name alone never authorizes deletion.

An interruption after the internal marker has been removed can strand an empty
directory. Cleanup preserves that remnant because it cannot verify both records;
it never reconstructs missing evidence to force a retry.

External evidence is removed only after successful candidate removal.
Ownership metadata storage itself is never recursively removed as part of run
cleanup. A retained parent can therefore contain metadata after all of its
eligible run directories have been removed.

## Lifecycle and stale cleanup

The lifecycle guard holds an internal ownership handle only after
initialization succeeds. Explicit completion, `Drop`, and unwinding submit
that handle to the shared authority. A refusal cannot fall through to
another deletion method. `Drop` reporting is best effort and never panics
or replaces an error already unwinding. `RuntimeGuard::remove` and
`RunWorkspace::finish` return `Result<()>`; preservation, unsupported guarantees,
and operational failures return a reason and the retained path. The run report
is finalized even when cleanup fails. The recipe caller propagates cleanup
errors, retaining an earlier execution error as the cause when both fail.

Dry runs also create an `OwnedRunDir` and use the shared authority for
explicit disposal, `Drop`, and unwinding. A `TempDir` destructor or another
pathname deletion must not bypass a refusal. Dry runs create ownership
evidence but no durable run report or recovery scope and do not invoke the
end-of-run sweep. Without a `run.json`, stale `run-tmp` discovery does not
collect their crash remnants; their prefix alone grants no authority.

Stale discovery retains existing run eligibility and process-liveness checks.
It carries the independently discovered root alongside each candidate;
discovery itself grants no deletion permission. Discovery calls
`OwnedRunDir::load` against the independently selected parent. Apply rechecks
the run report and liveness, then reuses the discovered ownership handle;
it does not call `load` again. The handle's `remove` operation rereads both
ownership records and revalidates them before removal.
An inactive run with a matching recorded path still survives if ownership,
protected-root, parent, or identity checks fail. The `run-tmp` category has
no age delay once a run is inactive and its directory is unused.

Durable run reports and `recovery/` state remain outside the disposable
directory. Worktree, Cargo target, archive, and unrelated temporary-leftover
policies remain documented in the
[cleanup guide](../howto/clean-up-after-recipe-runs.md#cleanup-categories).

## Internal API responsibilities

These are internal Rust responsibilities, not a public library API or a facility
for deleting arbitrary caller-provided paths.

| Component | Responsibility |
| --- | --- |
| `cleanup/owned_run_dir.rs` | Owns exclusive creation, evidence validation, protected observations, claims, and descriptor-relative removal. |
| Retained root descriptor and observation | Retain the independently selected root path, descriptor, mount and directory identity. |
| `OwnedRunDir` | Represents fully initialized creation authority; ownership constructors stay private. |
| Ownership record | Binds run, nonce, original leaf, root, and candidate identities. |
| Protected observation | Retains initial protections and refreshed environment observations. |
| `CleanupOutcome` | Distinguishes removal, verified absence, failed validation, unavailable guarantees, and operational failure. |
| `run_tmp_root.rs` | Selects external, default, or shortened parents without adopting existing leaves. |
| `run_dir.rs` | Stores ownership handles for durable and dry runs, routes every disposal through the authority, and supplies child-process environment values. |
| `sweep.rs` and `execute.rs` | Discover stale candidates and submit eligible candidates with their independent roots. |

The modules are under `crates/amplihack-cli/src/commands/cleanup/`.

| Internal operation | Contract |
| --- | --- |
| `OwnedRunDir::create(root, leaf, run_id)` | Accepts an independently selected parent and one fresh leaf name. Returns ownership only after exclusive creation and matching evidence; errors preserve incomplete initialization. |
| `OwnedRunDir::load(root, path, run_id)` | Loads matching creation evidence at the original leaf or its nonce-bound interrupted claim against a separately supplied root. Never creates missing evidence or adopts an unowned directory. |
| `OwnedRunDir::remove()` | Applies shared authorization and descriptor-relative removal; reports an outcome without expanding authority. |
| `OwnedRunDir::path()` | Reports the disposable location. A returned path is not a deletion capability. |
| `RuntimeGuard::remove()` and `RunWorkspace::finish(...)` | Return `Result<()>`, exposing cleanup refusals and failures to the caller while retaining the run report. |

## Outcomes and retained directories

| Outcome | Meaning |
| --- | --- |
| `Removed` | Authorized candidate removal completed; unrelated siblings and the configured parent remain. |
| `AlreadyAbsent` | A retained directory identity has been unlinked, with no original, claim, or external record remaining. A replacement at the original path is never treated as absence. |
| `Preserved(reason)` | Validation or claiming refused removal. Stale cleanup reports a skipped item; explicit completion returns an error. |
| `Unsupported(reason)` | A required platform or filesystem guarantee is unavailable. Remaining contents and available evidence are preserved; no weaker fallback is attempted. |
| `Failed(reason)` | An authorized operation failed; stale apply and explicit completion return an error. |

Unavailable platform or filesystem guarantees can cause initialization or
loading to fail, or removal to return `Unsupported(reason)`. There is no weaker
deletion method. Non-Linux creation permits execution but never grants removal
authority.

An operational failure after authorized traversal starts can leave partially
removed contents. Crash remnants can include an incomplete directory, claim,
or orphaned metadata; their existence never authorizes deletion. Legacy
directories may accumulate because cleanup does not manufacture missing
ownership evidence. Inspect retained paths and confirm that no process uses
them before considering manual removal. Do not add or repair markers to
force automatic cleanup.

Diagnostics contain reasons and relevant paths, not runtime secrets or record
contents. Explicit completion returns cleanup failures to its caller;
destructor reporting remains best effort and non-panicking. The contract protects against
accidental misuse, malformed evidence, and directory replacement. Cooperating
cleanup locks do not exclude arbitrary mutations by another process with the
same UID. These checks are not an operating-system security boundary against
code with the same filesystem authority.

## Validation status

The result-returning completion APIs, distinct outcomes, retained-parent handoff,
and nonce-bound claim recovery are implemented. These statements describe the
code contract; commit-specific test results and final review belong in the PR.
The regression contract below defines the evidence required before merge.

## Regression contract

### Guard commit before tests

Before running any tests for a cleanup safety change,
commit the shared deletion guard and its
integration at every runtime removal entry point: explicit completion,
`RuntimeGuard::Drop`, panic unwinding, stale apply, and dry-run disposal.
Review that commit for any remaining pathname recursion or automatic
temporary-directory destructor that could bypass the guard. Commit the
fixture isolation changes before executing the affected tests as well. Inspect
commit and push hooks before invoking them: a hook that runs tests is subject
to the same gate.
Do not run a destructive red-phase regression against an unguarded checkout.
Documentation edits and read-only or static checks do not satisfy this gate.

### Isolated fixtures and required observations

Validation starts with `TMPDIR` beneath a dedicated private validation
directory. Every executed test and applicable child process sets both `TMPDIR` and
`AMPLIHACK_WORKFLOW_RUNTIME_DIR` beneath their own private fixture directory;
neither points directly at `std::env::temp_dir()`. Fixture fallbacks never
escape into shared `/tmp`. Work, runtime-parent, temporary, and evidence
directories are isolated, with evidence outside disposable candidates.
Fixture creation errors fail explicitly. Tests use isolated child processes
or hold the shared environment lock across all environment reads, mutations,
and restoration. They restore the environment before fixture teardown,
including on unwind. Subprocesses and teardown helpers share the isolation
contract; a private outer launcher alone does not establish per-test isolation.

| Scenario | Required observation |
| --- | --- |
| Candidate is a private temporary root | Root and sentinel contents survive. |
| Unmarked or marker-only directory | Candidate and sentinel contents survive. |
| Missing, unreadable, malformed, symlinked, hard-linked, unsupported, or inconsistent records | Candidate survives. |
| Valid evidence but protected identity, alias, or wrong parent | Candidate survives independently of the ownership result. |
| Root or candidate replaced at a synchronized validation boundary | Replacement survives; authority is not transferred. |
| Claim replaced, interrupted, or blocked during restoration | Replacement entries survive; retained claims are reported. Retry requires matching evidence and the unchanged independently selected parent. |
| Initialization fails before authority is armed | Incomplete candidate is preserved and reported. |
| Valid owned direct child | Candidate is removed; siblings and their sentinels survive. |
| Explicit completion, Drop, unwinding, stale apply, or dry-run disposal | Each enforces the same authorization contract; refusal has no fallback deletion. |
| External roots of 59 and 60 resolved bytes with these prefixes | The first fits the 80-byte limit; the second is refused. Dry-run names fit any accepted external root. |
| Mount boundary or unavailable required primitive | Cleanup preserves the candidate when safety cannot be established. |

All three layouts retain coverage. Temporary-root and home-directory attacks
are represented entirely by isolated fixtures; no destructive case targets a
shared temporary root or the real home directory.
Replacement coverage uses deterministic synchronization or injected filesystem
operations. Tests never attempt recursive removal of shared host roots.

### Required validation evidence

Record results in the PR or CI logs, outside `docs/`. Each validation record
must include:

| Field | Required content |
| --- | --- |
| Guard commit | Commit SHA containing the reviewed deletion guard and all runtime entry-point integrations. |
| Tested revision | Commit SHA and any uncommitted differences, including fixture changes. |
| Invocation | Exact test or check command, test names, and exit status. |
| Environment | OS, filesystem, supported safety primitives, and any skipped platform-dependent cases with reasons. |
| Fixture roots | Private work root, selected parent, evidence location, `TMPDIR`, and `AMPLIHACK_WORKFLOW_RUNTIME_DIR` for the test and its child processes. |
| Observations | Expected and observed candidate, root, sibling, alias-target, and sentinel survival or removal; cleanup outcome and diagnostic. |
| Artifacts | CI log or retained evidence location, including synchronized replacement and mount-boundary checks. |
| Final review | Crusty's review of the final guard revision, blocking findings, and their resolution before returning the PR to the merge owner. |

Passing documentation link or whitespace checks is not implementation
validation. Evidence must cover all layouts and removal entry points,
including dry runs and path boundaries. Record the qualifying guard commit
separately from subsequent test invocations; an earlier candidate commit or
undated test result does not establish ordering. Commit subsequent safety
changes before validating them. The deputy retains merge authority.
