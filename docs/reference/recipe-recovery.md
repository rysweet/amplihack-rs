---
title: Recipe recovery reference
description: Durable recovery identity, persistence policies, resource declarations, and runner contracts.
doc_type: reference
last_updated: 2026-11-18
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
  - [Caller-set keys in routing and workflow recipes](#caller-set-keys-in-routing-and-workflow-recipes)
    - [Delivery controls across a relaunch](#delivery-controls-across-a-relaunch)
    - [What the tests observe](#what-the-tests-observe)
    - [What the lists do not cover](#what-the-lists-do-not-cover)
  - [Changing a persist list](#changing-a-persist-list)
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

Once the issue is known, amplihack writes it back to the recipe context in
both forms: `issue_number` as the number and, when `origin` is a GitHub
remote, `issue_url` as `https://github.com/<owner>/<repo>/issues/<n>`. The
runner treats every context key a shell step can see as an input of that
step, so a launch given the number and a relaunch given the URL must present
the same keys and values for completed shell steps to be replayed.

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

With the pinned runner, a step that reads an artifact-bound key executes again
after a relaunch even when the artifact is unchanged: the rebound path counts
as a changed input (`input_changed`). This is the open upstream bug
[amplihack-recipe-runner#218](https://github.com/rysweet/amplihack-recipe-runner/issues/218),
not intended behaviour. In `smart-execute-routing.yaml`,
`create-workstreams-config` rebinds `workstreams_file`, so
`launch-parallel-round-1` relaunches every workstream on each relaunch until
the pin moves to a runner with the fix. This is tracked in
[#2696](https://github.com/rysweet/amplihack-rs/issues/2696).

Shipped classification, preparation and design recipes opt into semantic
persistence. Container recipes re-enter their children; session setup and
authorization steps execute again. Declare any additional opaque file
dependencies before enabling reuse in custom recipes.
Session setup and authorization resources use current live admission;
saved session outputs alone are never sufficient. A user-supplied shell
command cannot act as a trusted live-resource rebinder.

### Caller-set keys in routing and workflow recipes

The pinned runner treats every context key as an input of every bash and
recipe step. A key that is set but missing from `persist.context` cannot be
saved, so every `replay: reuse` bash or recipe step in that recipe executes
again after a relaunch. A recipe that declares reuse must therefore list each
key that its caller sets with `-c` or that a parent passes in, not only the
keys its own steps read.

Six keys carry the caller's delivery choices: `base_branch`,
`closing_reference`, `forbidden_base_branches`, `pr_repository`, `repository`
and `strict_delivery`. The shipped routing and workflow recipes list them as
follows.

| Recipe | Caller-set keys in `persist.context` |
| --- | --- |
| `smart-orchestrator.yaml` | the six |
| `smart-classify-route.yaml` | the six |
| `smart-execute-routing.yaml` | the six |
| `default-workflow.yaml` | the six, and `step_recovery_dir` |
| `workflow-prep.yaml` | the six, and `step_recovery_dir` |
| `workflow-design.yaml` | the six, and `step_recovery_dir` |

The rule each row follows is that a recipe persists every key it declares or
can receive, not that every recipe carries an identical list. The three
routing recipes never receive `step_recovery_dir`: `auto-drive-to-merge.yaml`
creates the directory and `autodrive-build.yaml` hands the path straight to
`default-workflow`, so the key reaches the phase tree without passing through
classification or routing. None of the three declares it.

Each child lists every key its parent passes down. The set difference
`smart-orchestrator` minus `smart-classify-route` is empty, and so is
`default-workflow` minus either of `workflow-prep` and `workflow-design`.
Keeping those differences empty is what stops a child's `replay: reuse` steps
from becoming the restart point of every relaunch: the parent re-enters the
child on each attempt, and a child that cannot save a key it received
re-executes work it had already finished.

A key counts as an input whether the recipe declares it or a parent supplies
it. `base_branch` is declared with an empty-string default in five of the six
recipes, so it is always set, possibly to an empty value; `workflow-design`
does not declare it and receives it from `default-workflow`, which passes its
whole context into each phase sub-recipe. The same holds for
`closing_reference`, `forbidden_base_branches` and `pr_repository` there.
`strict_delivery` reaches the orchestrator from classification and then
reaches execute-routing. The delivery inputs `closing_reference`,
`forbidden_base_branches`, `pr_repository` and `repository` can be set with
`-c` or arrive from a development child; see
[delivery inputs](workflow-delivery-inputs.md).

`step_recovery_dir` is not a delivery input. `auto-drive-to-merge.yaml` creates
the directory through `amplihack orch helper step-recovery init` and binds the
path to the key, `autodrive-build.yaml` hands it to `default-workflow`, and
`default-workflow.yaml` declares it as an empty-string context default. No
phase step names the key in a `context:` block of its own; the key reaches each
phase recipe through `_execute_sub_recipe()`, which merges the whole context
into the child. (Two phase steps do carry a `context:` block, `workflow-publish`
clearing `commit_result` and `workflow-pr-review` clearing
`feedback_push_result`, and neither mentions `step_recovery_dir`.)

An empty value usually means step recovery is off for the run, and the steps
that consult it execute their body directly. `workflow-finalize.yaml` is the
exception: its first step runs `step-recovery init --created-by
workflow-finalize`, passing `--dir` only when the inherited value is non-empty,
and binds the result back to `step_recovery_dir` as its own output. An empty
inherited value there means the phase creates a directory of its own rather
than that recovery is off.

Two phase recipes use the value. `workflow-worktree` writes `build-branch.txt`
under it so a retried build can recover its branch, and `workflow-finalize`
reads it in four later steps, guarding `step-20b-push-cleanup`,
`step-21-pr-ready` and `validate-agentic-finalization`, then reporting with
`--remove-if-created-by workflow-finalize`. The
`--dir "$RECIPE_VAR_step_recovery_dir"` form appears in `workflow-finalize` and
in every autodrive recipe that wraps a step: `auto-drive-to-merge`,
`autodrive-build`, `autodrive-crusty-loop`, `autodrive-crusty-round`,
`autodrive-merge-loop` and `autodrive-merge-round`. The quoting is required,
because the value is a path chosen by the enclosing run.

No step in `workflow-prep` or `workflow-design` reads the key, and both persist
it regardless. That is the point of the entry rather than an oversight: the
pinned runner counts a context key as an input of every bash and recipe step
that can see it, whether or not the step mentions it. The key is set in every
`default-workflow` run, so a phase recipe that omitted it could replay none of
its `replay: reuse` bash or recipe steps.

Runner #208 widened input coverage for bash and recipe steps only, not agent
steps. In these two recipes the entry therefore rescues the bash reuse steps,
`step-02d-detect-host-type`, `step-03-create-issue` and
`step-03b-extract-issue-number` in `workflow-prep` and
`step-06b-checkpoint-doc-review` and `step-06d-goal-already-met-probe` in
`workflow-design`. The eight agent steps of `workflow-design` and the three of
`workflow-prep` replay on their own terms and do not depend on it. See
[step failure recovery](step-failure-recovery.md).

The other phase recipes are absent from the table for a different reason.
`default-workflow`, `workflow-prep` and `workflow-design` are the only three
with a top-level `recovery` block; `workflow-worktree`, `workflow-tdd`,
`workflow-publish`, `workflow-finalize` and the rest have none. None of their
steps is replayable, so they have nothing to persist and re-execute wholesale
on every relaunch, whichever keys they declare. `workflow-worktree.yaml`, for
one, declares `base_branch`, `forbidden_base_branches`, `strict_delivery` and
`step_recovery_dir` and takes their values from `default-workflow`, and a
persist list would make no difference to it.

#### Delivery controls across a relaunch

All six carry a delivery choice, and each defaults permissively.
`forbidden_base_branches` defaults to an empty deny list, `strict_delivery` to
`"false"`, and `base_branch` to the empty string, which leaves the base to the
remote default branch. `repository`, `pr_repository` and `closing_reference`
default to the empty string and are read only when strict delivery is on.

Recovery never restores a caller-supplied context value. A relaunch rebuilds
its initial context from the command line and the recipe defaults exactly as
the first attempt did; only the recorded context deltas of restored steps are
re-applied on top of that. Whatever drives the relaunch therefore supplies the
same `-c` values. A relaunch that drops `-c forbidden_base_branches=...` runs
with the empty deny list from its first step, whichever keys the persist lists
name. Re-supplying the delivery inputs is the caller's part of the contract,
not something a persist list can do for it.

Listing a delivery key therefore closes no fail-open hole, and omitting one
opens none. The permissive default is reached the same way before and after:
by a relaunch that omits the input. What a persist list governs is where a
relaunch restarts, with a second effect on which completions may be replayed.

That second effect is comparison. Persisted inputs are compared, never
restored: the runner records each referenced key's value, compares the saved
value against the live one, and reports `input_changed` when they differ, so
the dependent steps execute again under the live value. A step completed under
a strict value is therefore never replayed into a relaunch running under a
weaker one. Only a restored step's recorded output delta is re-applied, and
that comes from `persist.outputs`. A stale completion record cannot hand a
weaker delivery control to the steps that follow it.

One residual follows from that same mechanism. The strict-delivery checks live
inside `step-03-create-issue`, which calls `delivery_strict_mode` and, under
strict delivery, `select_strict_delivery_issue`. When a relaunch restores that
step, neither helper runs again: the issue identity is re-applied from the
record's output delta, so the checks are not re-evaluated against the relaunch's
own inputs. A changed delivery input is still caught, because the comparison
reports `input_changed` and the step executes again; an unchanged one is
accepted on the strength of the first attempt's check. Moving the checks out of
the restored step, so that they run on every attempt, is an open follow-up on
[#2711](https://github.com/rysweet/amplihack-rs/issues/2711).

The storage of the two kinds of input is asymmetric, which matters when
extending a persist list. Environment variables a step references are stored as
digests; context values named in `persist.context` are stored verbatim, with no
redaction pass. The lists above run to 93 or 94 keys each, agent outputs
included, so a recovery record holds a substantial picture of the run.
None of the keys above carries a credential, and a key whose value may contain
one does not belong in `persist.context`.

`repository`, `pr_repository` and `closing_reference` are written to private
recovery storage. Give them as `owner/name`, or as a plain issue reference,
never as a URL with credentials in it.

#### What the tests observe

The test
[`issue_2658_routing_replay.rs`](../../crates/amplihack-cli/tests/issue_2658_routing_replay.rs)
covers both levels on the pinned runner. Each case runs inside the fixture's
own temporary root, with a `HOME` and a `PATH` that hold no host agent and no
host runner, so only the fixture's fakes can answer an agent step, and with
`create_issue` and `strict_delivery` left at their defaults, so no step reaches
a remote.

The routing case runs the shipped `smart-classify-route.yaml`, unchanged, with
`base_branch=main` and `forbidden_base_branches=release/*`. A fake agent stops
the first attempt at `classify-and-decompose` with a recognised usage limit.
The relaunch restarts at `classify-and-decompose`, with `preflight-validation`
restored, and calls the fake agent exactly once more. Setting
`forbidden_base_branches`, rather than only the key the recipe declares, is
what makes the added entries observable: a key that is never set behaves
identically whether or not a persist list names it. `main` matches no pattern
in that deny list, so no shipped step changes what it does.

The phase case runs the shipped `workflow-prep.yaml` and `workflow-design.yaml`
as `replay: never` children of a parent recipe written into the fixture. That
parent stands in for `default-workflow` and supplies `worktree_setup` the way
the worktree phase would, which is what lets the design recipe's repository
resources resolve. The run's one added context input is `step_recovery_dir`,
which is not a delivery input, bound to a `0700` directory inside the fixture
root. Both children finish; the fake agent then stops the parent's trailing step
at a usage limit, so the children's bash steps are already complete when the
relaunch starts.

The relaunch restores the bash `replay: reuse` steps of both children:
`step-02d-detect-host-type`, `step-03-create-issue` and
`step-03b-extract-issue-number` from `workflow-prep`, and
`step-06b-checkpoint-doc-review` and `step-06d-goal-already-met-probe` from
`workflow-design`. All five appear in the relaunch's `restored_steps` and none
of them is the restart point; the only step the relaunch executes is the
parent's trailing one, and the fake agent's call count is unchanged, so no
agent step of either child ran a second time either. `resume_checkpoint` is left
unset so the probe's condition holds and the step runs on the first attempt.
Remove `step_recovery_dir` from either child's persist list and that child's
earliest bash reuse step becomes the restart point instead:
`step-02d-detect-host-type` for `workflow-prep`,
`step-06b-checkpoint-doc-review` for `workflow-design`.

Stopping at the parent's trailing step, rather than inside `workflow-design`,
is what puts `step-06d-goal-already-met-probe` under observation at all. It is
the last step of its recipe and sits after all eight of that recipe's agent
steps, so a usage-limit stop at one of those agent steps would leave it
unfinished, and no relaunch could restore it.

No test compares persist key lists. What a test observes is which steps a
relaunch restores, which is the behaviour those lists exist to produce.

#### What the lists do not cover

These lists cover the keys that callers set. They do not cover the keys that a
development child adds when `execute_sub_recipe` merges its whole final context
back into the routing recipe, defaults included. After a development child
finishes, the routing bash and recipe steps that follow it execute again on
relaunch. The same limit applied before the runner moved to 0.7.0 (revision
`2833a0b`). This is tracked in
[#2697](https://github.com/rysweet/amplihack-rs/issues/2697).

The merge-back also writes the child's values for `forbidden_base_branches` and
`strict_delivery` over the values the caller set, defaults included. A routing
step that runs after a development child therefore reads the child's values,
not the caller's. #2697 also covers stopping a child from weakening a delivery
control that the caller set.

### Changing a persist list

A persist list is part of the recipe file, so editing one changes the file's
digest. Recompute that recipe's entry in
[`_recipe_manifest.json`](../../amplifier-bundle/recipes/_recipe_manifest.json)
as the first sixteen characters of the file's SHA-256, which is what
`sha256sum <file> | cut -c1-16` prints. Recompute only the files you edited.
Entries for untouched recipes are left alone, including entries that already
disagree with their file.

This check reports whether a recipe file and its manifest entry agree:

```bash
python3 -I - <<'CHECK'
import hashlib, json, pathlib

root = pathlib.Path("amplifier-bundle/recipes")
manifest = json.loads((root / "_recipe_manifest.json").read_text())
names = ["smart-classify-route", "default-workflow", "workflow-prep", "workflow-design"]
for name in names:
    digest = hashlib.sha256((root / f"{name}.yaml").read_bytes()).hexdigest()[:16]
    print(name, "ok" if digest == manifest[name] else f"drifted ({digest})")
CHECK
# Output, with every entry in step:
# smart-classify-route ok
# default-workflow ok
# workflow-prep ok
# workflow-design ok
```

A `drifted` line for a recipe you did not edit is pre-existing and is left
alone; fixing it would change a bundle digest for no reason and hide the drift
you were looking for. Narrow `names` to the files you touched.

That truncated prefix detects drift between a bundle and its manifest. It is
not tamper protection: it is 64 bits wide, and installation checks only that
each entry is present and non-empty.

A removed key is checked by hand rather than by a committed test. Take a copy
of the recipe, drop one key from its `persist.context`, and run the matching
case against the copy. The relaunch then no longer restores that recipe's
`replay: reuse` bash and recipe steps, and the restart point moves back to the
earliest of them. Record the restart point the run reports; do not predict it
from the step list and write that down instead, because the earliest affected
step is not always the one a reader expects.

Two things make the prediction unreliable. Agent steps are not affected at all,
so the earliest `replay: reuse` step in the file is often the wrong answer; and
a step's `condition` can keep it from running in the case you are using. In
`smart-classify-route.yaml` the earliest reuse bash step is
`preflight-validation`; in `workflow-prep.yaml` it is
`step-02d-detect-host-type`, which precedes `step-03-create-issue`; in
`workflow-design.yaml` it is `step-06b-checkpoint-doc-review`, which precedes
the goal probe. Those are the steps to expect; take the restart point from the
run's own report.

The check is only meaningful for a key the run under test actually sets.
Dropping a key that the run never sets changes nothing, and the check then
reports a pass over a copy that is in fact broken. `smart-classify-route.yaml`
declares only `base_branch` and `strict_delivery` of the six; the other four
reach it through the orchestrator's context merge or an explicit `-c`.
`workflow-prep.yaml` and `workflow-design.yaml` declare no `step_recovery_dir`
at all: it reaches them from `default-workflow`, which is why the phase case
passes it on the command line. Confirm the key is set in the run you are using
before reading a result either way.

Automating the check would mean committing a recipe that is wrong on purpose,
or a test that reads persist lists instead of watching behaviour.

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

Creation explicitly requests `0700` for directories and `0600` for files.
Masks that preserve owner permissions, including `0002`, leave these modes
unchanged. A mask that removes owner permissions can leave new state
unusable; explicit private creation does not restore those permissions.

The store is reached through the work root, so the folders above it are
checked too. The runner opens the path one component at a time from `/`,
without following links, and creates any missing folder at `0700`. Each
folder above the work root must be a folder, not a link, with no group or
other write bit, or with the sticky bit set. The work root must be owned by
the effective user with permission bits `0700`; special bits follow the
runner's existing rules. Before the runner opens the store, `recipe run`
checks the path and names inspectable failures. An existing root at `0755`,
`0750` or `0775` stops execution without changing its permissions. A mode
diagnostic includes the path, actual mode and correction:

```text
work root /data/amplihack-work has mode 0755; fix it with `chmod 700 /data/amplihack-work`
```

A root or ancestor is never automatically changed. Writable ancestors name
their group/world write bits, actual mode and the appropriate `chmod g-w`,
`chmod o-w` or `chmod go-w` correction. Ownership failures name the observed
and expected UIDs and require correctly owned storage; chmod alone is not
presented as a solution.

If the runner fails later, read-only diagnostics inspect relevant current
storage, including `recovery-index/`, the current identity directory,
`<run dir>/recovery/`, documents and locks. A nonprivate child is identified
by its own path and mode, with `chmod 700` for directories or `chmod 600` for
regular private files. Inspection stops at links, wrong types and inaccessible
components. It does not scan historical runs, read document contents, replace
the runner's descriptor checks or discard its original error chain. If a
concurrent change prevents attribution, the message states that uncertainty.
See
[Work root permissions](recipe-command.md#work-root-permissions) for the full
rules and messages.

Changing a work root to `0700` yourself cannot undo earlier access or remove
entries another user created. The rule checks only the mode bits of folders
above the work root, not their owners: a folder above the work root that
belongs to another user can be renamed or replaced by that user even at
`0755`. Keep the work root under folders owned by you or by root.

Private state callers explicitly select private construction for
`AtomicJsonFile`, `FileLock` and `AtomicFlag`. Missing parent components use
`0700`; data, flags, lock files and publication temporary files use `0600`.
Generic constructors retain their existing storage contracts, and private
construction does not change existing parent permissions. Shared helpers
preserve their separate strict and legacy contracts; recovery roots and
ancestors never use a helper that changes their existing permissions.
Lock acquisition still precedes reading or modifying shared state; existing
lock waits, synchronization, atomic publication and temporary-file cleanup
are unchanged. Ordinary Git checkout contents retain repository permissions.

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
