---
title: Workstream state and recovery
description: Immutable identities, tracking acquisition, execution results and ownership during recovery.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workstream state and recovery

This reference describes native plan persistence, tracking and recovery.

## Contents

- [Saved plan and identity](#saved-plan-and-identity)
- [Tracking acquisition](#tracking-acquisition)
- [Execution transitions](#execution-transitions)
- [Resumption](#resumption)
- [Ownership and cleanup](#ownership-and-cleanup)

## Implementation boundary

The engine saves a canonical plan and per-workstream state beneath
`std::env::temp_dir()/amplihack-workstreams/state`. On Unix, select the root
with `TMPDIR`. Use the same root and execution repository when resuming.
`status --base-dir` selects an inspection location; `run` and `cleanup` use
`TMPDIR`, without separate resume or state-directory flags.

Runtime directories use mode `0700`; JSON state uses atomic replacement and
mode `0600`. A runtime lock prevents concurrent owners. Invalid or unreadable
state stops execution. Completed results are persisted before dependants start.
Source repository, plan, identity, route and execution-directory conflicts preserve existing artefacts
and return an error. The caller's Operations repository is never owned by the
runtime. These checks do not confine the agent's filesystem access.

The scheduler uses child-exit, cancellation and runtime deadline events.
`max_runtime` defaults to 7200 seconds. At the deadline, both supported policies
persist `timed_out_resumable` and retain recovery artefacts:

- `continue-preserve` (default) lets the owned child continue. Its eventual
  process result determines completion; an unsuccessful exit cannot be promoted
  by generated prose.
- `interrupt-preserve` interrupts only after output on both streams has been
  idle for `AMPLIHACK_IDLE_TIMEOUT_SECS` (default 300 seconds). Active output
  postpones interruption. The owned process group is killed and the child
  reaped; the outcome stays resumable, even if the child exits normally after
  crossing the deadline. Its dependants are blocked.

Output activity is tracked independently of capture size. Both output streams
are drained with bounded capture (`AMPLIHACK_MAX_LOG_BYTES`, default 100 MiB
per stream); truncation is reported and truncated runner results cannot prove
success. Git default-branch resolution and clone retain their established
30-second and 300-second bounds. This restores the logging and idle-watchdog responsibilities previously
provided by `log_output.rs` while retaining structured runner output.

Restored live processes and uncertain launch intents are preserved with an
actionable error, rather than signalled or relaunched. The engine cannot prove
completion for a process it no longer owns. Cancellation also stops and reaps
owned children and propagates blocking through every pending descendant.

## Saved plan and identity

Before external actions, save the normalised workstream array at
`<base>/state/canonical-plan.json`, where `<base>` is
`std::env::temp_dir()/amplihack-workstreams`. Select a separate stable
temporary root per plan; this prevents independent plans sharing that file.
On Unix, use the same `TMPDIR` for launch, inspection, resume and cleanup.
The canonical plan must survive recipe scratch-file cleanup. It is not the
helper's disposable `smart-orch-ws-*.json` file.

`state/repository.json`, every ownership marker and each workstream state bind
recovery to Git's canonical common directory, Git directory and source worktree
root. A shared remote URL is insufficient. Another repository or linked worktree
must use its own runtime root. Legacy state without this identity fails with
instructions to preserve the artefacts and select a fresh runtime directory.

Replacement files are uniquely created with private permissions in the
destination directory. The writer syncs the complete replacement, renames it,
and syncs the parent directory on Unix. Ordinary failures remove the temporary
file without replacing the original error. A retained `canonical-plan.tmp`
cannot prevent another write. Persistence failure stops subsequent launches.

Before execution, the orchestrator saves each normalised leaf's immutable
key, task, description, classification, resolved route, dependency keys and
tracking requirement. Runtime state records tracking acquisition, process
ownership, execution results and lifecycle transitions.

| Value | Persistence contract |
| --- | --- |
| Key | Always present, safe and unique; reused on resume |
| Tracking issue | Optional positive integer; omitted when absent |
| Route | Resolved before preparation; Operations remains `operations-direct` |
| Dependencies | Refer to immutable keys, independently of tracking issues |
| Results | Associated with the producing key and saved before releasing dependants |
| Execution directory | Supplied context for Operations; distinct from owned runtime artefacts |
| Ownership | Required before reusing artefacts or observing restored processes |

Initially tracked work retains its numeric key and existing artefact names,
such as `ws-2553` and `ws-2553.json`. Issue-less work receives a safe key once.
Acquiring or attaching an issue does not rename it. Resume reads the saved
identity rather than recomputing it from mutable descriptions or tracking.

Legacy saved numeric zero means absent tracking. New configuration rejects
zero. Conflicting saved identities or incompatible ownership are errors;
they are not ignored to manufacture a fresh identity.

## Tracking acquisition

This is per-leaf orchestrator acquisition, distinct from legacy
recipe-level `create_issue` context forwarding.

Acquisition applies only when `create_issue` is explicitly true and no valid
issue is present.

| Acquisition condition | Required action |
| --- | --- |
| Not requested | Execute without tracking |
| Existing issue present | Reuse it |
| Requested but not started | Save intent and a stable reconciliation reference before contacting the service |
| Confirmed success | Validate and persist the returned number before launching the affected leaf |
| Confirmed failure | Report the error; do not execute that leaf |
| Request outcome uncertain | Reconcile through the saved reference, or report an actionable unresolved error |

Recovery never blindly repeats an uncertain creation request. If issue
creation succeeds but result persistence fails, the saved intent and
reference remain the basis for reconciliation. Credentials are excluded from
plans, contexts, references and diagnostics.

## Execution transitions

Persistence failure must prevent additional launches and retain
recovery information. Private runtime directories and state files must have
restrictive permissions.

Completion and cancellation events drive transitions and scheduling. The
orchestrator waits for owned child completion, with receiver deadlines for
runtime and idle policies. It does not periodically probe process existence or
retry uncertain external writes.

A successful completion is persisted before capacity is reused and eligible
dependants launch. Each dependant receives the prerequisite result identified
by immutable key. Failed, partial or cancelled prerequisites visibly block
affected dependants. A non-zero process or runner exit remains a failure even
when the agent's text claims success.

Cancellation is associated with a workstream key and verified process
ownership. It preserves recoverable state and artefacts. It does not delete
the execution repository.

## Resumption

Resume uses the existing run command and the saved canonical plan. From the
same intended execution repository, select a stable root before initial run:

```bash
export TMPDIR="$HOME/.amplihack/runtime/operations-routing"
mkdir -p "$TMPDIR"
chmod 700 "$TMPDIR"
amplihack multitask run operations-workstreams.json
```

For this example the canonical plan is
`$HOME/.amplihack/runtime/operations-routing/amplihack-workstreams/state/canonical-plan.json`.
After interruption, select that same root, inspect state and resume the saved
array instead of regenerating the decomposition:

```bash
export TMPDIR="$HOME/.amplihack/runtime/operations-routing"
amplihack multitask status --base-dir "$TMPDIR/amplihack-workstreams"
test -r "$TMPDIR/amplihack-workstreams/state/canonical-plan.json" || exit 1
amplihack multitask run "$TMPDIR/amplihack-workstreams/state/canonical-plan.json"
```

Do not choose a fresh `mktemp` directory on resume or pass the `state/`
subdirectory as `--base-dir`. Preserve the canonical plan, `ws-<key>.json`,
progress sidecars, logs and owned runtime directories together. Do not rerun
until uncertain acquisition and live-process ownership have been resolved.

### Reconcile an uncertain tracking request

1. Read the affected leaf's acquisition intent, target repository and stable
   reconciliation reference from its saved state. Preserve that state and
   the canonical plan; do not issue another create request.
2. Use the authenticated tracking service to look up that exact reference
   in the target repository, including closed issues. Confirm the issue
   belongs to this request; title similarity alone is insufficient.
3. If exactly one issue matches, the reconciler validates its positive
   number and atomically saves the confirmed acquisition and tracking metadata
   before launch. Preserve the original key, paths and dependency references.
4. If no issue matches, stop for operator reconciliation; automatic creation
   is not retried after an uncertain result. If the result is
   ambiguous, unavailable or duplicated, keep the leaf unresolved and report
   the repository and reference for operator investigation. Do not launch it
   or guess an issue number.
5. Resume with the command above after the resolution is saved. The engine
   must reuse a confirmed issue rather than create another one. Do not remove the intent or change its reference to force another creation.

### Required resume decisions

| Saved condition | Resume behaviour |
| --- | --- |
| Completed leaf | Preserve the result; do not repeat its actions |
| Inactive, unfinished leaf | Resume with its saved key, route, dependencies and tracking state |
| Restored live leaf | Preserve state and report that restored completion observation is unavailable |
| Live leaf whose ownership or completion observation cannot be established | Report the limitation and preserve state and artefacts |
| Uncertain tracking acquisition | Reconcile before launching the affected leaf |
| Failed prerequisite | Keep affected dependants blocked until explicitly resolved |

A PID alone does not prove ownership. Platform-specific observation limits do
not permit signalling an unproven process or silently rerunning a live leaf.

### Current recovery limits

There is no automatic adoption of a saved live process, on either macOS or Linux.
Preserve its state, logs and runtime directory until the original process and
its external outcomes have been reconciled. A missing acknowledgement from issue
creation is reconciled on the next run using `tracking_reference`; zero or
multiple matches stop execution without creating a duplicate.

A recorded `launching` state is also uncertain: the child might have started
before a crash prevented the PID write. It requires reconciliation before a new
launch. The engine never treats corrupt state as a fresh run.

## Ownership and cleanup

Cleanup requires saved compatible ownership before deleting runtime artefacts.

Runtime artefacts are separate from execution directories. Operations uses
the supplied execution context without claiming ownership of that repository.
Development retains its established workspace preparation and delivery rules.

Reuse, process observation, signalling and deletion require compatible
ownership. Filesystem checks guard symlink substitution and path races;
a string-prefix check alone does not authorise deletion. Incompatible paths,
state or processes are preserved with a diagnostic.

`amplihack multitask cleanup` removes eligible owned artefacts. Its dry-run
shows the proposed removals. Development cleanup retains the existing merged-PR
conditions. Issue-less execution does not need a fabricated issue or PR to
participate in cleanup, and Operations cleanup preserves the caller's
repository.

## Related

- [Routing and optional tracking](dev-orchestrator-routing.md)
- [Multitask command](multitask-command.md)
- [Run Operations and mixed requests](../howto/run-operations-and-mixed-work.md)
- [Recipe usage-limit recovery](../concepts/recipe-usage-limit-recovery.md)

## Security enforcement and behavioural evidence

The Rust production-path fixtures in
`crates/amplihack-cli/tests/issue_2553_execution.rs` use private profiles, runtime
roots and Rust service adapters; they contact no production service.

| Requirement | Production enforcement | Behavioural coverage |
| --- | --- | --- |
| Validate a complete plan before actions | `plan::normalise`, `ownership::persist_plan`, `validate_saved` | Invalid keys, collisions, cycles, unsupported policies/recipes, conflicting saved routes and legacy identity reject without child execution |
| Bind recovery to its source | `RepositoryIdentity`, saved-state validation and ownership markers | Same-repository completion resumes once; a different repository cannot adopt it |
| Enforce Operations precedence | `plan::resolve_route`, typed `execution::launch` | Conflicting recipes and parent routing variables cannot request development preparation; actual edits receive a development route |
| Preserve caller files during cleanup | Private runtime lock, `ownership::verify`, terminal/reaped-state checks and canonical deletion containment | Unowned data, altered markers, foreign ownership and directory/marker symlinks are rejected; caller repository survives |
| Avoid PID-based authority | Live-state rejection; scheduler retains its own `Child`; `WNOWAIT` reserves PID until reaping | A forged saved live PID cannot be adopted, signalled or relaunched |
| Reconcile uncertain issue creation | `tracking::acquire` saves UUID intent before the authenticated `gh` call | Lost acknowledgements reconcile by exact marker; zero/multiple matches and authentication failures stop the affected leaf without another create |
| Treat results as data | Separate structured context and result fields; route and ownership never merge from output | Adversarial result fields and shell text cannot change the selected recipe or execute a shell command |
| Handle credentials and injection | Argv-only service/child invocation, private context files, filtered parent routing variables; tracking and cleanup errors omit raw service stderr; repository scope redacts URL userinfo; credential-bearing development origins fail before Git command construction | Fixture credentials never enter generated state, context, arguments or reported acquisition/cleanup errors; existing scope tests cover credential-bearing remotes |
| Propagate unsuccessful outcomes | Exit code and runner failure override prose; fixed-point blocking | Reverse-ordered chains and branching graphs block after failure, partial outcome, cancellation or unsuccessful timeout |

These checks enforce native routing and runtime ownership. They do not sandbox
an agent's tools, redact arbitrary secrets supplied as task data, or remove the
selected provider/GitHub authentication environment needed by authorised child
work. Runtime artefacts and agent output therefore remain private data. macOS
results do not establish Linux process behaviour; Linux/CI evidence is reported
separately in the implementation hand-off.
