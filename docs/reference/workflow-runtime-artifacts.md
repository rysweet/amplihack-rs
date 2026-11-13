---
title: Workflow runtime artefacts reference
description: Workspace validation, external launcher state and common-Git runtime exclusion contracts.
doc_type: reference
last_updated: 2026-11-12
---

# Workflow runtime artefacts reference

This reference describes candidate runtime boundaries for launchers, indexing commands,
default and consensus workflows, nested recipes and recovery helpers.

**Acceptance remains incomplete.** Required runner acceptance and historical
evidence are recorded separately from these implementation contracts.

## Contents

- [Implementation gaps](#implementation-gaps)
- [Workspace validation](#workspace-validation)
- [External state resolution](#external-state-resolution)
- [Launcher identity and readers](#launcher-identity-and-readers)
- [Checkout-local exclusions](#checkout-local-exclusions)
- [First-write preparation API](#first-write-preparation-api)
- [Shell helper API](#shell-helper-api)
- [Lifecycle integration](#lifecycle-integration)
- [Regression contract](#regression-contract)

## Implementation gaps

External launcher context and session records use checkout-specific directories.
`get_shared_runtime_dir` uses a separate clone-scoped directory for power-steering
state, preserving linked-worktree sharing. The owned exclusion catalogue includes
`/.amplihack/runtime/`, `/index.scip.backup` and indexing PID files. Registration
preserves existing rules and permissions and appends effective runtime rules
after later negations; repeated preparation is idempotent.

The [checkpoint reference](workflow-checkpoint-publication.md#implementation-gaps)
describes supported index formats and publication restrictions. The original
native runner controls and historical publication packet remain outstanding
acceptance evidence; source changes alone do not close those gaps.

## Workspace validation

Validation is read-only. It establishes an existing, non-bare checkout, its
canonical root and common Git directory, expected task location, and the remote
and base-history prerequisites for the requested workflow. It completes before
runtime preparation, checkout-local writes or worktree creation.

Ordinary launch never runs `git init`, including when `auto_init_repo=true` is
defaulted or supplied alone. A non-Git workspace, including one containing nested
checkouts, is refused without alteration. Any separately supported explicit
creation flow establishes all applicable preparation, remote and worktree
prerequisites before mutation.

## External state resolution

| Context | State location |
| --- | --- |
| Managed recipe run | The run's external runtime directory, supplied by the runner. |
| Standalone launcher | Identity-scoped state beneath the existing Amplihack external state root. |
| Child recipe or agent | The inherited run root and validated project, checkout and session identity. |

Recipe work-root configuration and retention follow
[Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md).
`AMPLIHACK_WORK_ROOT` selects the runner's work area. The runner sets
`AMPLIHACK_RUNTIME_ROOT` and `AMPLIHACK_WORKFLOW_RUNTIME_DIR` for its steps;
caller-supplied `AMPLIHACK_RUNTIME_ROOT` does not override that selection.

| Setting | Role |
| --- | --- |
| `AMPLIHACK_WORK_ROOT` | External work area selected before recipe launch. |
| `AMPLIHACK_RUNTIME_ROOT`, `AMPLIHACK_WORKFLOW_RUNTIME_DIR` | Run locations supplied to child steps by the runner. |
| `AMPLIHACK_RUNTIME_DIR` | Explicit launcher-state base; must pass external-placement validation. |
| `AMPLIHACK_HOME` | External state root when suitable; can separately identify framework assets. |

A launcher-state override remains identity-scoped and cannot alter clone-sharing
contracts. Children use the run's validated location consistently; conflicting
or checkout-local overrides fail rather than creating a second state collection.

Without a run root, standalone launchers use the
[external state-root convention](hook-state-root.md): a suitable external
`AMPLIHACK_HOME`, otherwise `$HOME/.amplihack`. A checkout used as
`AMPLIHACK_HOME` remains an asset source and is not used for state. There is no
fallback to the current directory or `REPO_PATH`.

Effective state paths are absolute and private. Resolution checks existing
ancestors and symlinks and rejects a runtime override resolving inside the
target checkout, its Git metadata or any other registered checkout of the clone.
An invalid override is reported, not silently treated as
external. Launcher prompt-file storage is validated before writing; an explicit
external runtime directory also supports launch when `HOME` is unset. Missing usable external state is an error for dependent launcher
writes. Read-only resolution creates no directories; preparation revalidates
containment and ownership before and after creation, using private directories
and owner-only files. A path that changes identity during preparation is refused.

Standard run directories include `locks/`, `logs/`, `metrics/`, `provenance/`
and `reflection/`. Internal identity directory names are not a user-configurable
interface.

## Launcher identity and readers

External state contains `launcher_context.json`, `sessions.jsonl` and
`sessions.jsonl.lock`. Other launcher locks and temporary replacement files
remain external too. Context replacement is atomic; session logging retains
locking and compaction.

Project identity derives from the canonical common Git directory. Checkout
identity derives from the canonical worktree root. Session keys cannot escape
their directory and distinguish original IDs that would collide after simple
character replacement. Records retain the original identity metadata; corrupt
or mismatched records are rejected.

Writers and readers share resolution: launcher context, `agent-binary`, CLI
environment resolution, both session trackers, both nesting detectors and
shared-runtime lookup all use the external identity. Independent repositories
with the same directory name or session label do not share context. Linked
worktrees share project identity but retain distinct checkout identity.

| State | Identity scope | Linked-worktree behaviour |
| --- | --- | --- |
| Launcher context | Clone and checkout | Each checkout selects its own context. |
| Session records and locks | The owning session collection | Writers, readers and locks resolve the same external collection. |
| Shared power-steering state | Clone | All linked worktrees resolve the same external directory. |

`get_shared_runtime_dir` returns a clone-scoped external directory. Launcher
context remains checkout-specific. Moving the files preserves which callers
share them. Session identity is retained within the relevant collection;
independent clones never share records merely because their names match.

Agent selection retains precedence: explicit binary override, live session
markers, user-configured binary, tmux-server-held values, validated persisted
context, then the built-in default. Relocating context does not elevate it above
live session evidence. Legacy `<checkout>/.claude/runtime/launcher_context.json`
is never a routing or nesting authority. Readers do not recreate it or search
unrelated ancestor checkouts for a replacement.

## Checkout-local exclusions

Every owned checkout-local writer first registers exclusions in:

```text
<canonical git-common-dir>/info/exclude
```

The common directory is resolved through Git, not by assuming `.git` is a
directory. The same exclusions apply to the main checkout and linked worktrees
without user or system Git configuration.

The ownership catalogue covers these locations and their owned sidecars.

| Root-anchored location | Contents |
| --- | --- |
| `/.amplihack/graph_db` | Graph data, whether represented by a file or directory. |
| `/.amplihack/kuzu_db` | Legacy graph database storage. |
| `/.amplihack/graph_db.wal`, `/.amplihack/graph_db.lock`, `/.amplihack/kuzu_db.wal`, `/.amplihack/kuzu_db.lock` | Database sidecars. |
| `/index.scip`, `/index.scip.backup` | Retained framework-generated root index output, where applicable. |
| `/.amplihack/indexes/` | Framework-generated indexes. |
| `/.amplihack/index.scip`, `/.amplihack/index.scip.backup` | Framework SCIP output and backup. |
| `/.amplihack/indexing.pid` | Indexing process state. |
| `/.amplihack/blarify.json` | Framework-generated graph input. |
| `/.amplihack/runtime/`, `/.amplihack/locks/`, `/.amplihack/session-state/` | Owned runtime and locks. |
| `/.amplihack/metrics/`, `/.amplihack/classification_state/` | Owned metrics and classification state. |
| `/.claude/runtime/` | Any remaining local runtime, including locks, metrics and classification state. |
| `/worktrees/` | Workflow-owned nested worktrees. |

Graph storage remains checkout-local. Framework-generated root `index.scip` and
its backup are redirected into owned index storage where consumers support it;
any retained root output is registered before writing. Additional owned cache,
index, lock or temporary paths enter the catalogue before their writers run.
Ownership includes sidecars beside a database file, not just descendants of a
directory.

The catalogue does not blanket-ignore `/.amplihack/` or `/.claude/`.
`.amplihack/config.toml`, `.claude/settings.json`, agents and skills remain
visible. Existing user-authored index files are not reclassified as runtime
merely because their basename matches generated output.

Registration serialises cooperating writers and replaces the exclusion file
atomically. It preserves existing bytes, comments, permissions and an
unterminated final line, separating appended patterns with a newline. Equivalent
effective exclusions are not appended twice. Matching observes Git ignore
semantics, including anchoring, file/directory forms and later negations. A
comment or ineffective earlier pattern is not proof of coverage.

Registration failures are reported before dependent directories, databases,
PID files, locks, metrics or temporary files are created. There is no global
ignore or tracked `.gitignore` fallback.

## First-write preparation API

These internal interfaces separate path descriptions from validated Git
operations. Their handles are opaque; callers cannot manufacture mutation
authority from a path string.

| Module | Inputs | Result and side effects |
| --- | --- | --- |
| `amplihack_types::paths::owned_runtime` | Owned artefact kind | Pure local path and exclusion description; no Git subprocesses. |
| `amplihack_git::launcher_state` | External root and canonical project, checkout and session identities | Pure identity-bound paths; no directory creation. |
| `amplihack_git::workspace_validation` | Requested root and workflow prerequisites | Opaque validated checkout handle, or an error before mutation. |
| `amplihack_git::runtime_preparation` | Validated handle and owned runtime locations | Fallible, serialised common-exclusion registration before dependent writes. |

Standalone indexing commands use the same preparation boundary as workflows.
Callers cannot replace a validated handle with an arbitrary path or ignore a
preparation error and continue writing.

## Shell helper API

The installed helper remains
`amplifier-bundle/tools/workflow_runtime_artifacts.sh`. Recipe helper lookup
checks `WORKFLOW_RUNTIME_ARTIFACT_HELPER`, the explicit `AMPLIHACK_HOME` bundle,
the target or active checkout's bundle, then installed Copilot and Amplihack
bundles. Missing helpers fail visibly; a target need not vendor the framework.

| Function | Contract |
| --- | --- |
| `cleanup_known_workflow_runtime_artifacts(worktree)` | Narrow owned-leftover cleanup; preserves tracked source and the index. |
| `preflight_known_workflow_runtime_artifacts(worktree)` | Cleanup and lifecycle validation; never authorises broad staging. |
| `record_run_created_branch(repo, branch)` | Idempotent run-scoped branch manifest in the common Git directory. |
| `cleanup_run_created_branches(repo, intended_branch)` | Preserves intended, checked-out and protected branches; remote changes use validated publication. |
| `cleanup_nested_worktrees(repo)` | Removes owned nested worktrees with Git deregistration, preserving siblings and the main checkout. |
| `finalize_workflow_runtime_artifacts(task_worktree, intended_branch)` | Finalises only the dedicated task worktree's owned resources. |
| `finalize_workflow_cleanup_entry(repo_root, worktree_path, intended_branch)` | Confirms a distinct linked task worktree before nested cleanup; never falls back to committing in the launch checkout. |

Cleanup preserves a main checkout's `worktrees/` and other runs' live worktrees.
Failed worktree discovery preserves the directory and reports the failure.
Tracked source, symlink escapes and unproven ownership prevent deletion. Missing
owned paths are no-ops. Cleanup does not silently remove protected staged work.

## Lifecycle integration

Preparation precedes the first write; cleanup is not the exclusion mechanism.
Checkpoints, hook repair, review feedback, publication and finalisation use the
[bounded checkpoint and publication contracts](workflow-checkpoint-publication.md).
The required `auto_stage: false` setting applies to reachable default, consensus
and autodrive agent roles, including `autodrive-crusty-round` steps `02` and `04`.

Finalisation commit and guard steps require the actual linked task worktree.
They skip visibly if that worktree is missing or unsuitable, and never fall back
to the launch checkout. The strict runtime rule in
`artifact-guard --mode pre-publish` applies before any launcher-file exemption.

## Regression contract

Fixtures use disposable repositories without ignore rules and isolated profile
roots configured before any profile lookup. Launch acceptance unsets `HOME` and
`XDG_CONFIG_HOME` and supplies external `AMPLIHACK_HOME`, Claude and Codex roots.
Fixtures never use the real profile roots. Synchronisation uses child completion,
readiness signals or isolation, without timers, interval probes or timed retries.

Launch checks compare `git ls-files --others --exclude-standard` with the same
source manifest before and after launch, with `HOME` and `XDG_CONFIG_HOME` unset
through launch and inventory collection; an explicit external run or state root
is supplied. A repository-owned inventory hook continues to pass with its
original source manifest. Coverage includes every location above plus launcher
context, session logs and their lock. Linked-worktree controls cover shared
exclusions, existing content, idempotence and registration failure before writes.

Publication and runner acceptance have separate requirements in the
[checkpoint reference](workflow-checkpoint-publication.md#regression-and-runner-acceptance).
This contract describes required behaviour, not a record of executed checks.
