---
title: Recipe Checkout Isolation
last_updated: 2026-11-10
doc_type: reference
status: planned
owner: workflow-team
---

# Recipe checkout isolation

> **[PLANNED - Implementation Pending]** This reference specifies the intended
> contract. Isolation, recording, artifact paths and lifecycle extensions below
> are unimplemented requirements, not shipped guarantees. Implementation and
> validation are pending; present-tense statements describe required behavior.

Every Git-backed recipe run has a private execution checkout before its first
shell or agent step. Every auto-drive also has a private review checkout.
The caller's checkout is a read-only source of inputs. These boundaries apply
to foreground, detached, nested and concurrent runs.

## Contents

- [Execution inputs](#execution-inputs)
- [Paths and environment](#paths-and-environment)
- [Private review generations](#private-review-generations)
- [Command records](#command-records)
- [Ownership and lifecycle](#ownership-and-lifecycle)
- [Configuration and failures](#configuration-and-failures)
- [Reproduction evidence](#reproduction-evidence)

## Execution inputs

Preparation resolves the caller's HEAD and creates a unique detached linked
worktree at that commit. A branch name, PR number or matching revision never
allows a run to reuse another checkout. A nested invocation gets a new checkout;
a retry belonging to the same run can retain that run's checkout.

Preparation reproduces tracked edits and untracked inputs without changing the
caller's files, index or HEAD. Staged and unstaged changes remain distinct:
when a file has staged content followed by further edits, the private index
contains the staged content and the private working file contains the later
edits. Filenames and file content are captured without text conversion.
Source fingerprints are verified before execution so a concurrent caller edit
cannot silently produce a mixed snapshot.

Preparation fails explicitly for unborn HEAD, unresolved index entries,
unsupported index flags, dirty submodules, special files, unsupported symlinks,
required excluded inputs that cannot be reproduced safely, or a source that
changes during capture. It never silently omits a required input or repairs the
caller's repository to proceed.

Outside a Git repository, recipes keep their existing execution behavior. The
run records that Git checkout isolation is inapplicable. A Git discovery error
inside a repository is a failure, not permission to execute in the caller.

## Paths and environment

Repository-local recipe inputs, typed context paths such as `repo_path`, and
step working-directory overrides are mapped to the corresponding locations in
the execution checkout. Relative paths retain their meaning within that tree.
Mutable destinations that lead back into the caller's checkout, including
symlink escapes, are rejected. A caller checkout path can authorize reading;
it cannot authorize staging, committing, resetting, checking out or pulling
there. Arbitrary context strings are not rewritten as if they were typed paths.

Validation must resist filesystem replacement between checking a path and using
it. Canonicalization and symlink rejection alone are insufficient. Preparation,
path mapping, metadata access and cleanup must use descriptor-relative no-follow
operations, or ancestors protected against replacement for the entire operation.
The same protection must cover every mutable destination through use, including
Git administrative paths. If the platform or launch mechanism cannot establish
that protection, the operation fails closed before mutation.

Inherited Git routing and configuration injection are sanitized during
repository discovery, checkout creation and child launch. In particular,
`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and inherited configuration overrides
cannot redirect execution to the caller's index. Intentional identity and
authentication remain available. Required recording configuration reaches shell
steps, agents and hook descendants across runner environment allowlists.

Linked worktrees share objects, refs, configuration storage and administrative
metadata. Worktree registration and necessary administrative updates are
permitted; caller HEAD, index, working files and shared configuration are not
execution outputs. No run adds caller exclusions or changes caller Git
configuration as a repair step.

This is execution separation, not an operating-system sandbox. An arbitrary
same-user process can still write a path it has filesystem permission to write.
Path validation and command records do not make hostile child code harmless.

## Private review generations

A drive resolves and records the intended PR head SHA, then creates its own
unique detached review checkout at that SHA. Concurrent drives of the same PR
have separate directories and indexes. Each new PR-head generation has its own
checkout and baseline; a drive never repoints another generation in place.

Trusted launch-side recipes, reviewer instructions, tools, hooks and agent
configuration remain outside the reviewed tree. Reviewer launch excludes
PR-controlled instruction and configuration discovery, including repository
`AGENTS.md`, `CLAUDE.md`, plugins and settings. A CLI launch path that cannot
establish this boundary is refused before review.

The guard baseline is established before judging starts and binds ownership,
repository, PR head and generation to the checkout. It is verified after
judging, round completion and evaluator completion, including failure paths.
Index or HEAD mutations inside that private directory still produce
`REVIEW_DIR_CHANGED` and stop the drive. The guard is never relaxed to tolerate
such writes. Mutations in the caller or another drive's checkout do not alter
this baseline.

Fixes and publishing use a separately owned execution checkout; judging does
not commit in the detached review checkout. The publisher names the intended
remote branch and revalidates ownership and generation before publishing.
The execution checkout may remain detached: branch checkout is not proof of
authority. Trusted ownership metadata binds its repository identity, PR number,
validated remote identity, full destination `refs/heads/` ref, expected PR head
SHA and review generation. Fix commits descend from that recorded head (including
any permitted base merge). Before publishing, the drive revalidates the binding,
checks the current remote PR head still matches the expected head and verifies
that the proposed commit is a fast-forward descendant. It pushes the explicit
execution commit SHA to that full destination ref, without relying on an upstream
or checking out the PR branch. A changed target stops publishing and requires
reconciliation and a new review generation; a server-side non-fast-forward
rejection also stops the drive. Hooks run normally; pushes are never forced.
Published evidence remains bound to its exact commit SHA.

## Command records

Required recording starts before executable steps and is finalized before a
run can succeed. Coverage includes index-writing Git commands in shell steps,
agent descendants and hooks, including absolute executable invocations and
alternate indexes. A parent launch log or a PATH wrapper alone does not provide
this coverage.

The planned durable layout (not yet emitted by the runner) is rooted at `AMPLIHACK_WORK_ROOT`, defaulting to
`~/.amplihack/work`:

| Location relative to the work root | Contents |
| --- | --- |
| `runs/<run-id>/run.json` | Run identity, parent correlation, isolation applicability, ownership and diagnostics pointers |
| `runs/<run-id>/diagnostics/git/commands.jsonl` | Normalized command records |
| `runs/<run-id>/diagnostics/git/raw/` | Restricted original observation events |
| `runs/<run-id>/diagnostics/git/capabilities.json` | Recorder capability probes, supported coverage and demonstrated limitations |
| `runs/<run-id>/diagnostics/git/finalization.json` | Collection outcome, failures and finalization state |

Each normalized record carries a schema version, run identity, timestamp,
command and executable evidence, effective cwd, resolved index path and Git
routing environment, including whether routing variables were absent. Parent
and child run identities permit correlation without sharing checkout ownership.
An index path is resolved for the observed command's effective Git context,
including command-line routing and alternate indexes; it is not guessed from
the launch cwd. A record identifies an index a command could write, not proof
that every invocation changed its bytes.

Initialization verifies attribution capabilities for the installed Git and
runner combination. Missing required coverage blocks launch. Collection or
finalization failure makes the run fail and leaves diagnostics available even
if recipe steps otherwise succeed. Completion requires draining observation
events; timer-based probes and timed retries do not establish completeness.

Recording is diagnostic evidence. A descendant that removes observation
configuration, a non-Git program writing index bytes directly, or unavailable
process-observation facilities can limit attribution. Capability reports state
what was actually demonstrated; they do not claim universal or tamper-proof
coverage. A known gap in required coverage is a failure, not a successful run
with a quiet omission.

Raw events and argv can contain secrets. Diagnostics have owner-only access;
restricted originals remain separate from sanitized reports. Publishable reports
redact credentials and identify any resulting attribution gaps.

## Ownership and lifecycle

Execution checkouts and review generations are registered in the existing run
ownership records before child launch. Versioned metadata is published atomically;
partial preparation and recorder failures remain inspectable. Supplying an
arbitrary checkout path does not establish ownership.

The [existing cleanup lifecycle](../howto/clean-up-after-recipe-runs.md) removes
only eligible owned worktrees through sanitized Git operations. Active runs,
live descendants, dirty files and unpublished commits preserve recovery work.
Detached commits must be reachable from a retained published ref before their
checkout is disposable. Unreadable or unsupported ownership metadata keeps the
checkout for inspection. Cleanup never force-removes a checkout or deregisters
another run's worktree.

Command records and reproduction evidence live outside disposable `runtime`
and short temporary directories. Removing those directories or an eligible
checkout does not remove its diagnostics. Retaining evidence is independent of
retaining the working files needed for recovery.

## Configuration and failures

Isolation and required recording are automatic for Git-backed runs. There is
no switch to reuse the caller checkout or downgrade recording to best effort.
`AMPLIHACK_WORK_ROOT` selects the managed storage root under the existing
ownership and permission rules. Relocating that root does not migrate old runs.
Diagnostics are located through the run record, not by assuming a global log
folder or the current directory.

| Failure | Result |
| --- | --- |
| Inputs cannot be reproduced or verified | No executable step starts; caller remains unchanged |
| Mutable path escapes isolation or replacement protection is unavailable | Operation rejected before mutation; no fallback to caller cwd |
| Recorder cannot initialize required coverage | Execution blocked; partial diagnostics retained |
| Recorder fails during execution or finalization | Run fails, including when steps return success |
| Review launch can load PR-controlled instructions | Review blocked |
| Review ownership, target or baseline fails verification | Drive stops; private evidence retained |
| Cleanup cannot prove work is disposable | Checkout retained with a reason |

These facilities are internal runner and drive contracts. They introduce no
public Rust API. Internal prepare, baseline and verify operations accept trusted
ownership references, validate schema versions and return one versioned JSON
envelope on stdout; paths alone do not grant mutation authority.

## Reproduction evidence

A complete candidate-1 reproduction uses disposable repositories and profile
roots: a linked worktree, an untracked file in the main checkout, and a commit
in the linked worktree running the real applicable hooks. Replaying only
`git add` does not exercise commit hooks or the pre-commit stash/restore path.

The retained manifest identifies fixture and hook revisions, exact commands,
relevant environment, command records, hook output, and before/after caller
index bytes and HEAD. Every fixture child, including hooks, uses disposable
HOME and tool-profile roots with no fallback to real `.claude`, `.amplihack`,
`.codex` or the user's profile. Synchronization uses isolation or explicit
events, never sleeps or wall-clock test windows.

An observed caller-index writer is named only when recorded evidence supports
that attribution. If no caller-index write occurs, the report states that
result and leaves historical attribution unproven. Historical hypotheses are
kept separate from observations. Reproduction reports and validation results
belong in retained evidence and the PR, not in this reference. Linux-specific
`/proc`, pidfd and FIFO-pool coverage belongs in CI; the PR identifies coverage
that could not run on macOS.

## Related documentation

- [Run a recipe](../howto/run-a-recipe.md)
- [Recipe runner architecture](../concepts/recipe-runner-architecture.md)
- [Recipe runner logging](recipe-runner-logging.md)
- [Auto-drive reference](auto-drive-to-merge.md#where-phases-2-and-3-run)
