---
title: Recipe Run and Auto-Drive Checkout Isolation
description: Planned input preservation, private Git state, ownership, recording, and PR-head review contracts.
doc_type: reference
last_updated: 2026-11-10
---

# Recipe run and auto-drive checkout isolation

**[PLANNED - Implementation Pending]** This is the target contract, not a
statement of delivered runtime guarantees. All contracts below remain planned
until the production-boundary evidence in the [requirement and review ledger](#requirement-and-review-disposition-ledger)
is accepted.

The working-tree implementation now provisions private repositories, preserves
separate staged and working contents, and gives independent launches fresh
ownership. It also adds required Linux runner-descendant observation and
exact-commit review generations with separate trusted launch directories.
This candidate is **not delivered or accepted**. The detailed contracts below
remain targets where broader coverage has not been established. In particular,
local-input declarations, preparation/release observation, agent instruction
discovery, durable dependency recovery, and full hook acceptance still need
evidence. See the [implementation evidence](../operations/evidence/issue-2096-step8-implementation.md).

Current operating guidance remains in [recipe usage](../howto/run-a-recipe.md#know-where-the-run-works),
[cleanup](../howto/clean-up-after-recipe-runs.md#what-happens-when-a-run-ends),
[logging](recipe-runner-logging.md#git-command-record), and
[auto-drive](auto-drive-to-merge.md#where-the-judging-agents-start).

## Planned isolation contract

Each independent invocation owns an execution checkout, index, command record,
and cleanup lease. Preparation finishes before the first recipe step, agent,
or execution hook starts. The caller's checkout is a read-only source: the run
does not stage, commit, switch branches, reset, pull, or repair Git
configuration there.

## Contents

- [Execution inputs](#execution-inputs)
- [Required local-input declarations](#required-local-input-declarations)
- [Requirement and review disposition ledger](#requirement-and-review-disposition-ledger)
- [Paths and Git configuration](#paths-and-git-configuration)
- [Independent and nested runs](#independent-and-nested-runs)
- [Ownership and lifecycle](#ownership-and-lifecycle)
- [Required recording](#required-recording)
- [Private review generations](#private-review-generations)
- [Configuration](#configuration)
- [Rust component contracts](#rust-component-contracts)
- [Failure behavior](#failure-behavior)
- [Validation contract](#validation-contract)
- [Security boundary](#security-boundary)

## Execution inputs

`amplihack recipe run` resolves `--working-dir` (`-w`, default: the current
directory) before launching the runner.

| Invocation | Execution behavior |
| --- | --- |
| Inside Git | A detached execution worktree backed by a private repository; the same relative subdirectory becomes the runner's working directory. |
| Confirmed outside Git | The requested directory, subject to the recipe's Git preconditions. Required recording still applies. |
| Missing Git, corrupt metadata, or an unreadable repository | An explicit preparation failure, never a fallback to nonrepository operation. |
| `--dry-run` | No recipe steps execute and no execution checkout is provisioned. This does not certify observer capability or successful execution. |

The planned input snapshot preserves these layers independently:

| Input | Preserved meaning |
| --- | --- |
| `HEAD` | The caller's starting commit. |
| Index | Staged additions, modifications, and deletions, including objects not reachable from a commit. |
| Working tree | Unstaged contents and deletions, even when they differ from the staged version of the same path. |
| Untracked and ignored inputs | Files required by the recipe, including local recipe and context files. Ignored status alone does not justify omitting a required input. |
| Paths | Path bytes, symlinks, and explicit invocation paths retain their meaning or the invocation is rejected. |

Consistency checks detect caller changes that prevent a coherent capture.
Unsupported index states, unsafe symlink destinations, or required inputs
that cannot be represented safely fail preparation with the input and a
corrective action. An unborn `HEAD` requires a first commit before running a
Git-based recipe. There is no silent fallback to committed content or the
caller's checkout.

Preserving a local file does not make it trusted reviewer configuration. See
[Private review generations](#private-review-generations).

## Required local-input declarations

**[PLANNED - schema proposal; not accepted by the current runner]** Recipes
must declare filesystem dependencies before launch. The CLI resolves and
validates the declaration before snapshotting; the runner must support the
same schema version. A declaration that an older runner would ignore must
instead fail its capability check. This proposal requires schema and adapter
review before implementation; it does not add a working YAML option.

The proposed top-level recipe field is `execution_inputs`:

```yaml
# [PLANNED] Input declaration for the future acceptance tutorial.
execution_inputs:
  version: 1
  required:
    - local-notes.txt
  context_paths:
    repo_path: repository
  opaque_path_fields: []
```

| Field | Proposed interpretation |
| --- | --- |
| `version` | Required integer `1`; unknown versions or fields fail validation. |
| `required` | Explicit file or directory paths relative to the source repository root, independent of `-w`'s subdirectory. No absolute paths, parent traversal, glob, shell, or environment expansion. Missing or unreadable entries fail preparation. Directories select their descendants; ignored files require selection just like other untracked files. |
| `context_paths` | Context key to path kind: `repository`, `file`, or `directory`. Resolve relative values from the caller's requested working directory; validate kind and map caller-local paths to private equivalents. External files/directories are copied into owned input storage and the typed value is mapped there; external mutable repositories are refused unless a durable-worktree adapter validates ownership. |
| `opaque_path_fields` | Context keys whose free-form strings carry filesystem dependencies that cannot be represented as a typed path. A trusted adapter registered for the recipe identity and context key must enumerate inputs and produce mapped structured values before launch; without one, preparation fails naming the key and asking for typed inputs. Never rewrite prose by substring replacement. |

Committed files, staged additions, and working changes to tracked/indexed
paths belong to the baseline snapshot. Untracked and ignored files outside
that baseline are copied only when selected by `required` or a typed path.
A typed directory selects its full subtree. The CLI automatically includes
the explicit recipe file, recursively resolved local sub-recipes, and
`--context-file`/`--context-from-file` payloads; it does not assume that paths
mentioned inside those payloads have been declared. Recipe files outside the
repository keep their resolution relationships in owned input storage.
Sub-recipe declarations are merged before launch. Dynamic sub-recipes must
be resolved by a capability-checked adapter or rejected before steps start.

Capture keeps staged bytes separate from working bytes. It records the
selection and source-to-execution mapping without persisting secret input
contents as metadata. Regular files, executable bits, and safely contained
symlinks retain their meaning. Absolute or escaping symlinks, special files,
unsafe destinations, path collisions, unsupported path bytes, and concurrent
source changes that prevent a coherent snapshot cause a named preparation
failure. No silent omission or dereference into caller state is permitted.
The implementation must use byte-safe Git enumeration, not newline splitting.

### Detecting unsupported path semantics

Detection happens at structured boundaries: CLI path arguments, declared
context keys, sub-recipe resolution, and adapter inputs. Validation rejects
undeclared keys used by known file/path consumers, unknown path kinds,
missing required paths, and nonempty `opaque_path_fields` without a compatible
adapter. The diagnostic identifies the recipe, context key or path, and the
required declaration or adapter. A legacy recipe lacking declarations cannot
claim local-input compatibility; it needs migration review before acceptance.

Arbitrary shell commands and natural-language prompts cannot be scanned
reliably to discover every path. Undeclared context is prose, not permission
to consume a caller-local path. Recipe authors must enumerate filesystem
dependencies, including those embedded in command literals or agent tasks;
recipes with unresolved dynamic path dependencies are unsupported. Runtime
observation of a later access cannot satisfy a before-launch input check.
Adapter validation and recipe migration tests must establish this contract;
there is no claim of automatic opaque-path discovery or OS-level confinement.

## Paths and Git configuration

Discovery, snapshotting, provisioning, execution, validation, and release use
repository-bound Git commands. Inherited `GIT_DIR`, `GIT_COMMON_DIR`,
`GIT_WORK_TREE`, `GIT_INDEX_FILE`, object-directory overrides, injected
`GIT_CONFIG_*` values, and global/system configuration overrides cannot
redirect those commands to another checkout or index.

Preparation does not execute caller hooks, templates, clean/smudge filters,
or other untrusted executable Git configuration. Required execution hooks,
including ordinary commit and push hooks, run in the intended checkout under
recording supervision. Preparation-time suppression does not authorize
bypassing execution hooks.

Explicit recipe paths, context-file paths, `repo_path`, and `-w`
subdirectories are mapped before launch. A caller-local mutable repository
path resolves to the corresponding private input. Passing the caller's
absolute path as `repo_path` does not authorize writes there. Durable task
or PR worktrees have separate validated ownership and remain the destination
for intended branch commits.

Opaque context text is not rewritten by string substitution. If a required
path's meaning cannot be preserved safely, the invocation fails before
execution with an actionable explanation.

The private repository receives local objects without hardlinks or alternates
pointing back to the caller's object store. Caller indexes, refs,
configuration, and worktree-registration metadata are not shared execution
state. A caller directory in correlation output identifies the source; it
does not necessarily identify the runner's execution directory.

## Independent and nested runs

Independent runs always have different repositories, checkouts, indexes,
recording state, and cleanup leases. This also applies to an independently
launched invocation inside another run's process tree.

Internal nested execution can reuse a checkout only within the same logical
run while preserving isolation. `AMPLIHACK_RUN_CHECKOUT` is a hint, not proof
of ownership. Process ancestry, detached `HEAD`, or a matching path alone
never grants reuse or release rights.

Retries retain logical-run correlation, separate attempt identities, and
earlier evidence. A later successful attempt cannot erase an earlier
recording or publication failure.

## Ownership and lifecycle

Rust holds a non-cloneable checkout lease. Persisted ownership includes a
versioned schema, logical owner, generation, repository identity, canonical
path, and filesystem identity. Unknown ownership-critical schema values and
incomplete records cannot authorize deletion.

Private repository metadata lives outside disposable runtime storage. A
durable branch worktree registers its dependency before temporary execution
resources are released. Metadata remains until every registered dependent
worktree can be safely released.

Release revalidates ownership and filesystem identity at the destructive
boundary. It refuses substituted paths, symlinks, wrong generations, caller
resources, and another active run's resources. A killed run can leave
resources for ownership-aware recovery; age or location alone never
authorizes deletion.

Descendants are stopped and reaped before final recording publication.
Observation stays active through Git-based release. Required release or
publication failures prevent complete success and retain available evidence.
A general cleanup sweep's warning policy does not weaken mandatory
finalization.

Execution checkouts are disposable. Publish useful commits and output through
the recipe's durable branch worktree or retained artifact destination before
release. Runtime storage must not hold the only copy of work to keep.

## Required recording

Recording covers Git activity capable of writing an index across the pinned
runner, shell steps, agents, and hook descendants. A supervised Linux process
observer establishes coverage before permitted provisioning subprocesses
and remains active through teardown and Git release. Fork, clone, exec,
absolute Git paths, and reduced child environments are part of this boundary.
Setting parent Trace2 variables alone does not establish coverage.

Each relevant command carries executable, argv, launch directory, repository
and index attribution, relevant Git routing, time, run identity, and attempt
identity. Missing required attribution is a recording failure. A recorded
command capable of writing an index does not prove that the index changed.

Initialization, observation loss, collection, attribution, bounds,
persistence, synchronization, or finalization failure prevents complete
success. Live collection failures trigger controlled teardown. Unsupported
or denied observer capability rejects execution before the runner starts;
there is no advisory-only fallback.

The retained journal is `<run dir>/git-commands.jsonl`. An empty journal is
valid only with established coverage and no relevant activity. Trace2 can
supplement observation but cannot certify coverage by itself. See the
[Git command record](recipe-runner-logging.md#git-command-record) and retained
[Trace2 investigation](../operations/issue-2096-recording-probe.md).

## Private review generations

Each independent auto-drive owns a review generation bound to the repository,
PR number, and exact PR-head SHA recorded for that attempt. Rust provisions
its detached checkout at that SHA, fetching into owned storage when needed.
Another drive on the same PR gets separate resources. Main's `HEAD` is never
a substitute for the recorded PR head.

| Material | Authority |
| --- | --- |
| Files and diff being judged | The recorded PR-head checkout and commit. |
| Reviewer recipes, skills, prompts, settings, plugins, and launch hooks | Explicitly trusted material outside the reviewed checkout. |
| PR-controlled `AGENTS.md`, `CLAUDE.md`, `.claude/`, and nested instructions | Review data; never automatically loaded as reviewer instructions or configuration. |

Starting in an external directory is insufficient by itself. The agent
adapter prevents instruction and configuration discovery from the reviewed
tree, including during file inspection. Missing trusted material or an
adapter unable to preserve this boundary stops launch. Reviewed files are
not overwritten to make them appear trusted.

Before judging, the driver checks repository, owner, generation, and exact
`HEAD`. The #1528 index and `HEAD` guards cover judging, complete rounds,
before-round work, and loop-health evaluation, including failure paths.

- Confirmed mutation stops with `REVIEW_DIR_CHANGED`.
- Inspection failure stops with `REVIEW_DIR_UNSTAMPED`; it does not claim
  a mutation occurred.
- Invalid identity or untrusted launch inputs stop before judging.

Every verdict and round identifies the reviewed SHA. `reviewed_head_sha`
identifies the judged commit; `head_sha` can identify a later post-fix tip.
A PR advance requires a new generation. Old results do not certify the new
tip. GitHub calls carry validated repository identity rather than inferring
it from the trusted launch directory.

## Configuration

**[PLANNED - Implementation Pending]** These are existing setting names with
their target meanings. Isolation and recording will be required, with no
opt-out flag. Current runner and cleanup behavior remains narrower as
described in the linked operational guides.

| Setting | Meaning |
| --- | --- |
| `-w`, `--working-dir` | Selects the source and its relative execution location. |
| `-c repo_path=...` | Repository input, validated and mapped before mutable execution. |
| `--context-file`, `--context-from-file` | Local context inputs whose semantics are preserved. |
| `AMPLIHACK_WORK_ROOT` | Managed storage; see [cleanup settings](../howto/clean-up-after-recipe-runs.md#settings). |
| `RECIPE_RUNNER_RS_PATH` | External runner, subject to capability and supervision requirements. |
| `AMPLIHACK_RUN_CHECKOUT` | Execution context hint; setting it grants no ownership. |
| `AMPLIHACK_CLEANUP_ON_EXIT=0` | Skips the general sweep, not required release or recording finalization. |
| Caller `GIT_TRACE2_*` | Cannot select the evidence destination or disable required observation. |

## Rust component contracts

These are internal responsibilities, not a new public library or network API.

| Component | Contract |
| --- | --- |
| Git command policy | Binds operations to the intended checkout, index, and configuration policy. |
| Input snapshot | Captures independent committed, staged, and working states, maps paths, and rejects unsafe or inconsistent inputs. |
| `RunCheckout` and ownership | Provisions private resources and releases only resources authorized by its lease and durable dependencies. |
| `GitRecord` and Linux observer | Establishes descendant coverage, reports live failures, and explicitly returns finalization errors. |
| Review generation | Binds exact PR source, trusted launch material, guards, verdicts, and release to an owned generation. |
| `RunScope` and `RunWorkspace` | Combines execution, retries, cancellation, teardown, release, recording, and publication outcomes without losing simultaneous errors. |
| Recipe compatibility adapters | Invoke Rust operations; YAML does not implement provisioning or cleanup in shell. |

## Failure behavior

| Failure | Result |
| --- | --- |
| Unsupported input or unsafe path | Preparation failure naming the input and corrective action. |
| Unsupported observer or failed initialization | Runner does not start. |
| Collector, attribution, or storage failure | Controlled teardown, retained partial evidence, overall failure. |
| Wrong review SHA, owner, or generation | No judging or publication from that state. |
| Required release or final publication failure | Overall failure even if every recipe step succeeded. |

Existing table, JSON, YAML, stderr progress, and correlation fields retain
their meanings. Structured evidence and failures are additive. A runner exit
code of zero alone is insufficient: the final result includes required
recording, teardown, release, and publication outcomes.

## Validation contract

Regression coverage exercises the production CLI, pinned external runner,
agent adapter, and applicable hook boundaries. A substitute runner or a
manually created worktree tests only that narrower boundary.

| Boundary | Required evidence |
| --- | --- |
| Caller preservation | First-step staging leaves caller index bytes, `HEAD`, files, and configuration unchanged; staged and unstaged inputs remain distinct. |
| Concurrent and nested launches | Explicit rendezvous proves overlap; independent resources and release ownership remain separate. |
| Recording | Shell, absolute Git, Rust agent, and hook descendants are attributed through the pinned runner; initialization, live collection, persistence, and finalization failures cannot report success. |
| Review | Exact recorded SHA, trusted launch discovery, generation changes, mutation checks, and failed inspection are exercised through the drive entry point. |
| Real hooks | A linked-worktree fixture runs the applicable pre-commit framework and project hook stages, hygiene guard, staging, commit, and push to a local bare remote. Caller index/HEAD snapshots, hook output, and attributed commands are retained. |
| Lifecycle | Failure, cancellation, durable dependencies, and substituted ownership cannot delete another run's resources or lose available evidence. |

Fixtures isolate reads and writes with disposable repositories, homes,
configuration, and explicit profile roots. They never use the real
`~/.claude`, `~/.amplihack`, or profile root. Concurrency uses pipes, barriers,
or acknowledgements, with no sleeps, timers, or elapsed-time assertions.
The real-hook replay demonstrates present behavior; it does not identify a
historical writer without connecting evidence. Execution receipts belong in CI logs or the PR. The ledger below records
requirement disposition and links evidence; it does not turn fixture checks
into production acceptance.

## Requirement and review disposition ledger

**Disposition: changes required.** Target contracts are architecturally aligned;
implementation and production-boundary acceptance remain blockers. Source links
show the current behavior, not a passing acceptance result. The
[historical review](../operations/evidence/issue-2096-step17e-review.md)
records its own revision and must not be read as a review of later working-tree
changes. The [Trace2 investigation](../operations/issue-2096-recording-probe.md#review-follow-up-validation)
retains the narrower experiments and failed collection/failure guarantees.

| Requirement | Evidence or outstanding blocker | Review disposition |
| --- | --- | --- |
| Preserve `HEAD`, index, working tree, and required untracked/ignored inputs | Rust snapshot transfers staged blobs and copies working contents independently. Dirty/binary/byte-path/untracked/explicit ignored recipe contracts pass. | Broader required ignored inputs and external recipe dependencies remain unaccepted; the declaration proposal is not implemented. |
| Preserve explicit paths; reject opaque/unsafe inputs before launch | Caller-local absolute context paths map into execution; embedded caller paths and escaping symlinks reject launch. | Typed external paths and complete opaque dependency handling remain open. |
| Isolate repository metadata and preparation configuration | Private transport clones avoid caller hardlinks, alternates, and worktree registration. Hook/filter/config-injection contracts pass. | Local configuration includes and executable extension coverage require further review. |
| Separate independent/nested launches and retry evidence | Fresh leases replace marker-based checkout reuse. Socket barriers establish overlapping launches. Native observer scopes keep per-attempt journals. | Nested observer acknowledgement and pinned-runner results are recorded in the implementation evidence; retry/cancellation breadth remains open. |
| Enforce ownership, durable dependencies, cancellation and safe recovery | Release checks manifest and filesystem identity, returns errors, and retains metadata for durable worktrees. Substitution and durable-worktree contracts pass. | Descriptor-relative destructive operations and durable metadata reclamation remain open. |
| Attribute descendant Git through runner, agents and hooks | Native Linux successful-exec observation replaces Trace2 in production. Absolute cleared-environment Git and Rust-hook contracts pass. | Named Git executables in the observed runner tree only; actual agent discovery and preparation/release coverage remain open. |
| Fail on recorder initialization, live loss, storage, bounds or finalization errors | Required storage, observer loss, scoped receipts, bounded journals, and final publication are fallible. Fault-injection contracts pass. | Additional simultaneous failure and cancellation evidence remains required. |
| Pin review source to exact PR head with trusted instruction discovery | Rust generation provisioning preserves reviewed instruction bytes, validates the recorded SHA, and creates a separate launch directory. | Actual agent configuration discovery remains unvalidated; no trusted-launch acceptance claim. |
| Preserve #1528 mutation and failed-inspection distinctions | [Operational guards](auto-drive-to-merge.md#where-the-judging-agents-start) distinguish `REVIEW_DIR_CHANGED` from `REVIEW_DIR_UNSTAMPED`; generation-bound production replay remains required. | Retain existing checks; target integration pending. |
| Combine runner, recording, release and publication outcomes | Explicit release, collection, and run-report publication errors now prevent success and retain available evidence. | Broader retry/teardown and cleanup-error composition remains open. |
| Establish real-hook caller preservation and incident evidence | [Historical follow-up](../operations/evidence/issue-2096-step17e-review.md#validation-and-review-disposition) leaves the project-hook replay and historical attribution incomplete. [Real-hook validation](#validation-contract) remains required. | Outstanding; controlled fixture hooks are insufficient. |
| Demonstrate the tutorial's expected results | [Future acceptance exercise](../tutorials/recipe-checkout-isolation.md) specifies staged/working/untracked preservation. Shell syntax, fixture setup and recipe-YAML checks do not execute or certify the production boundary. | Pending; expected output is not a passing receipt. |

Remove implementation-pending notices only after linked production-boundary
receipts establish the corresponding guarantees and review accepts them.

## Security boundary

Private repositories isolate Git state; they are not OS sandboxes. A full-tool
process can still access other paths writable by its user. Observation is
scoped to the supervised process tree and supported capabilities; it does not
attribute unrelated host processes or arbitrary non-Git index writes.
Restricted files and redaction do not make evidence tamper-resistant against
the same user. Historical attribution needs connecting evidence; a fixture
replay alone cannot supply it.

## Related

- [Run a recipe](../howto/run-a-recipe.md#know-where-the-run-works)
- [Inspect a local-input snapshot](../tutorials/recipe-checkout-isolation.md)
- [Auto-drive execution and review](auto-drive-to-merge.md#where-phases-2-and-3-run)
- [Recipe runner architecture](../concepts/recipe-runner-architecture.md)
