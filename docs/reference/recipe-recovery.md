---
title: Recipe recovery reference
description: Durable recovery identity, persistence policies, resource declarations, and runner contracts.
doc_type: reference
last_updated: 2026-11-11
---

# Recipe recovery reference

The Rust runner records and replays compatible completed steps across launches.
Amplihack supplies work identity, durable storage, launch settings, current
admission and trusted failure classification. Recovery requires
`durable_recovery_v1` from the selected runner.

## Contents

- [Upstream schema boundary](#upstream-schema-boundary)
- [Launch controls](#launch-controls)
- [Identity and admission](#identity-and-admission)
- [Recipe persistence policy](#recipe-persistence-policy)
- [Step replay policy and resources](#step-replay-policy-and-resources)
- [Completion contract](#completion-contract)
- [Compatibility and invalidation](#compatibility-and-invalidation)
- [Private storage and publication](#private-storage-and-publication)
- [Delegation and outcomes](#delegation-and-outcomes)
- [Capability and dependency contract](#capability-and-dependency-contract)

## Upstream schema boundary

The managed executable is pinned by
[`claude-plugin/recipe-runner.rev`](../../claude-plugin/recipe-runner.rev).
The CLI also uses the same upstream library's recovery contracts and secure
storage primitives. The upstream [recovery contract](https://github.com/rysweet/amplihack-recipe-runner/blob/main/RECOVERY.md)
defines the opt-in YAML policies and private host integration. The legacy
`recovery_on_failure` agentic option and step-ID checkpoints remain separate.

## Launch controls

Recovery controls:

| Control | Behavior |
| --- | --- |
| Default execution | Selects only the latest matching predecessor; restores compatible progress only after a durable usage-limit stop and loss of the previous owner |
| `recipe run --fresh` | Bypasses historical replay throughout the new tree; preserves history and records the new attempt normally |
| `recipe run --dry-run` | Prints the plan without recovery admission, completion writes, or latest-pointer changes, including with `--fresh` |
| `recipe run --detach` | Passes identity, effective settings, and freshness through the private launch payload; only the executing process admits the attempt |
| `-c resume_checkpoint=...` | Retains existing checkpoint semantics; does not select a historical run or reconstruct its outputs |

No recovery database or separate service is required. `AMPLIHACK_WORK_ROOT`
selects the existing durable work root. `--fresh` is a per-launch flag,
not a persistent configuration switch. Recovery adds no usage-limit timer
or change to the existing same-process wait/retry policy.

## Identity and admission

| Identity part | Normalization and isolation |
| --- | --- |
| Recipe | Resolved recipe identity; execution definitions are checked separately for compatibility |
| Repository | Canonical Git common directory and repository identity; linked worktrees share identity, independent clones do not |
| Work | Positive issue number within that repository for `smart-orchestrator`; explicit `issue_number` or a structurally validated `issue_url` |
| Nested invocation | Parent call-site path, child recipe, stable branch/workstream discriminator, and iteration discriminator |

An issue URL must use an approved issue host, name the selected repository,
and contain a positive issue number. Conflicting or unverifiable work
identifiers do not enter a shared recovery namespace. The run logs the
reason and executes fresh. Task prose is not parsed as recovery authority.

An identity lock serializes predecessor selection, monotonic sequence
allocation, new-attempt registration, and latest-marker publication.
Sequence overflow is an error. The latest marker names the admitted
attempt even if it never reaches a completed step. Only after selecting
that marker does admission inspect the predecessor's outcome and owner.
Live ownership is established through existing process/session ownership
checks, not elapsed time. An active predecessor is not a recovery source.

Here, a **recovery attempt** is a durably admitted launch. Existing
same-process runner retry/correlation IDs remain distinct and do not
replace the launch's ordering sequence or recovery identity.

## Recipe persistence policy

Recipe-level `recovery` declares which values may be saved. Absence of this
policy permits execution but does not permit semantic context or output
persistence. Such work is not replayable. Child recipes declare their own
policies; a parent cannot grant permission to persist a child's values.
Parent result persistence also respects the policies of any child values
it contains.

| Field | Type | Meaning |
| --- | --- | --- |
| `recovery.version` | integer | `1`; unknown versions are not replayable |
| `recovery.persist.context` | list of strings | Exact context keys whose typed effective values and changes may be persisted; default empty |
| `recovery.persist.outputs` | list of strings | Step IDs whose complete structured results may be persisted; default empty |

Permission covers the entire value, including nested fields. It is an
explicit declaration that the value is suitable for private durable
storage, not automatic secret detection. No wildcard captures the
environment, credentials, authentication caches, or unrelated files.
Redacting a consumed value does not make an equivalent replay possible.
If required inputs, results, or context changes cannot be saved under the
policy, the affected step and its ordered suffix within that invocation
execute again.

This recipe illustrates recording the selected repository revision
from the `amplihack-rs` checkout. Its policy explicitly permits persistence of the revision output.

```yaml
name: recovery-revision-check
context:
  repo_path: "."
recovery:
  version: 1
  persist:
    context: [repo_path, revision]
    outputs: [read-revision]
steps:
  - id: read-revision
    type: bash
    command: git rev-parse --verify HEAD
    output: revision
    recovery:
      replay: reuse
      resources:
        - kind: repository
          path: "{{repo_path}}"
```

The Git command itself produces the checkout's full commit ID followed by a
newline. Under this contract, a successful launch is not eligible
for historical replay; eligibility still requires a later durable
usage-limit outcome in the same identified execution tree.

## Step replay policy and resources

| Field/value | Meaning |
| --- | --- |
| `recovery.replay: reuse` | Reuse a finalized result only after all compatibility and resource checks pass |
| `recovery.replay: never` | Execute the step again and invalidate its ordered suffix within that invocation; the default when no replay policy is declared |
| `recovery.replay: rebind` | Establish current live resources through a trusted runner integration before reusing dependent semantic work |
| `recovery.resources` | List of explicit dependencies in addition to the automatically captured execution definition, effective inputs, and settings |

Resource paths are resolved with the step's effective context and working
directory. They must identify concrete resources, not an unrestricted scan
of a user's directories. Each declaration grants validation or copying
authority only; it never grants deletion authority.

| `kind` | Required properties | Validation and restoration |
| --- | --- | --- |
| `repository` | `path` | Canonical common directory, revision, relevant index/worktree changes, and cumulative observations from completed steps |
| `file` | `path` | Relevant regular-file contents and metadata; absence or change invalidates reuse |
| `artifact` | `path`, `context_key` | Copy required bytes into private attempt storage; verify the copy and rebind the named path-valued context key before use |
| `live` | `name` | Trusted integration re-establishes the named resource's current ownership/admission; an unknown integration prevents reuse |

An artifact's `context_key` must be permitted by `persist.context`. Only an
explicit path-valued binding is rewritten. Paths embedded in opaque output
text cannot be rewritten safely and invalidate reuse if no longer valid.
Artifacts outside the run may instead remain external validated file
dependencies. Resources that cannot be copied or verified make dependent
results non-replayable.

Shipped classification, preparation and design recipes opt into semantic
persistence. Container recipes re-enter their children; session setup and
authorization steps execute again. Declare any additional opaque file
dependencies before enabling reuse in custom recipes.
Session setup and authorization resources use current live admission;
saved session outputs alone are never sufficient. A user-supplied shell
command cannot act as a trusted live-resource rebinder.

## Completion contract

Version 1 completion records carry the following logical fields. Their private
format is runner-owned, not an API for editing or fabricating checkpoints.

| Record content | Required invariant |
| --- | --- |
| Version and compatibility metadata | Supported schema, runner recovery contract, and canonical encoding |
| Attempt and invocation identity | Matches the admitted source, recipe, subtree, branch, and iteration |
| Step identity and execution definition | Covers command/prompt, conditions, ordering, context overrides, hooks, and relevant child definitions |
| Effective inputs and settings | Preserve types and all values relevant to execution under the persistence policy |
| Structured result | Original finalized success or validated skip, including output values and original result semantics |
| Context delta | Typed assignments and explicit removals; assigning null is distinct from deleting a key |
| Resource observations | Repository progression, file fingerprints, artifact bindings, and required live admission |
| Completion tree | Child coverage and order, including completed children beneath incomplete parents |

Completions are acknowledged only after durable publication. Parsing,
context propagation, required hooks, and other finalization must succeed
before a successful step is committed. A failed hook prevents recording its step; existing hook failure semantics
remain unchanged. Partial output and temporary publications are not completions. A skipped condition is revalidated
against compatible inputs before its saved skip can be reused.

Restoration uses the existing executor, including normal sub-recipe context
merging. It restores structured values and original results rather than
creating synthetic success strings. Completed side-effect hooks are not
repeated. Live authorization and resource admission are checked afresh.

## Compatibility and invalidation

Fingerprints use versioned canonical SHA-256 encoding. Map ordering is
normalized, list ordering and scalar types remain significant, and
unambiguous field boundaries prevent concatenation collisions.

Compatibility includes execution-relevant definitions and child coverage,
effective inputs, agent/provider and execution settings, hook policy,
repository revisions and worktree observations, declared files, artifacts,
and live authorization. A change to any required observation invalidates
the affected result. Changed recipe behavior invalidates reuse even when
its human-readable version string is unchanged.

Launch IDs, timestamps, and private transport paths are bookkeeping only
when they are not semantic recipe inputs. Operational values consumed by
commands or prompts remain inputs and cannot be hidden from comparison.

Repository validation accounts for cumulative mutations made by completed
steps. A recorded workflow branch creation or commit does not by itself
invalidate earlier work. Unexplained changes, including changes left by an
interrupted step, must be verified or cause invalidation. Recovery never
resets the repository to satisfy an old fingerprint.

The first incomplete or incompatible step must execute. Its result and
every subsequent result in that affected invocation must be invalidated
in execution order, even if a later result appears independently compatible.
Invalidation propagates through dependent enclosing results. A changed child
prevents reuse of a completed parent that covered it; re-entering the parent
can still reuse compatible child prefixes. Independent parallel branches
and iterations retain their own invocation identities and ordered progress;
they are preserved unless their own compatibility checks fail or they depend
on invalidated work.

## Private storage and publication

Durable state and copied artifacts live in `<run dir>/recovery/`, outside
disposable `runtime/`. Admission metadata and latest-attempt markers live
in private identity-index storage under the work root. Internal filenames
are not a stable interface. `run.json` is a summary, not replay authority.

Directories are owner-controlled `0700` and files are regular `0600`, or
platform equivalents. Traversal, reads, locks, and temporary files reject
unsafe ownership, permissions, links, and object types. Artifact copying
rejects devices, sockets, FIFOs, and unsafe links, confines destinations to
the admitted attempt, and hashes copied bytes while preventing substitution.

A single writer creates an exclusive temporary file in the destination
directory, serializes the complete bounded record, synchronizes it,
renames it atomically, and synchronizes the directory. Only that completed
publication counts. Readers reject duplicate security-relevant JSON keys,
invalid relationships, duplicate records, unsupported versions, oversized
documents, excessive nesting, and overflowing counters. State, artifact,
and message limits are checked before allocation or import.

Each new attempt imports a self-contained copy of inherited candidates and
required artifacts before executing work. Candidates remain subject to
validation. Repeated recovery does not depend on an ancestor's retained
directory. An import racing removal of historical detail falls back fresh. The identity
marker survives deletion of detail and prevents resurrection of older history.

Unverifiable historical state causes a clearly logged fresh fallback;
missing resources invalidate affected results. Unsafe current storage,
failed admission, or failed durable publication stops execution. No
temporary-storage fallback may claim durable recovery. Retention must preserve
latest-attempt tombstones even when detailed records are removed. The
retirement command and its coordination interface remain outside this change; existing
`amplihack cleanup` categories do not provide recovery retirement. Before
exposing that interface, it must serialize retirement with admission and
imports, exclude active owners/imports, and durably preserve the latest
sequence/tombstone before removing detail. It must cover inherited sensitive
copies without deleting external resources. See
[recovery retention](../howto/clean-up-after-recipe-runs.md#preserve-or-retire-recovery-state).

## Delegation and outcomes

The private launch descriptor binds the admitted attempt, identity,
storage, predecessor, effective settings, and freshness policy. Trusted
delegated processes inherit a private local IPC attachment rather than
saved context or credentials in arguments. The root owns the only writer.

The server enforces attempt, invocation subtree, operation, and generation
permissions. Children cannot write sibling records or complete the root.
Stale updates, forged attachments, oversized frames, and conflicting
repeated commits are rejected. Repeated identical commits receive
idempotent acknowledgments. Handles are closed before unrelated subprocess
launches, and outstanding requests are bounded.

Usage-limit evidence comes only from trusted classifiers, runners, or
supervisors. The blockage is published promptly before waiting or unwinding.
Status revisions and generations serialize updates; retry advances the
generation and revokes the old blockage before new work begins. Success
or non-limit failure supersedes earlier blockage. If the process disappears
with a still-current durable blockage, that evidence remains eligible once
ownership is gone. Disappearance after retry without new durable limit
evidence is an unknown outcome and starts fresh. A child cause propagates
through failed parents, but an old child limit cannot independently make
the enclosing run eligible. Status and diagnostics expose bounded reason
codes, source run, and restart path without context values or IPC secrets.

## Capability and dependency contract

The integration requires `durable_recovery_v1` from the selected
runner, including for `--fresh`. Partial support must not advertise this
capability. Runner changes are delivered through the supported upstream dependency mechanism;
the Amplihack pin advances only to merged upstream support, with existing
runner ancestry validation retained. There is no vendored second executor
or non-Rust recovery implementation.

Validation enforces this recovery capability; see the
[enforcement](recipe-runner-validation.md#recovery-capability-enforcement).

See [runner validation](recipe-runner-validation.md),
[recovery result metadata](recipe-result.md#recovery-metadata), and
[runner architecture](../concepts/recipe-runner-architecture.md#durable-recovery-ownership).
