---
title: Recipe runner architecture
description: Execution, supervision, and durable recovery ownership across Amplihack and the Rust runner.
doc_type: explanation
last_updated: 2026-11-12
---

# Recipe Runner Architecture

Why the recipe runner is an external binary, how amplihack locates and invokes
it, and how recipe execution stays predictable.

## Contents

- [Why external](#why-external)
- [Binary resolution](#binary-resolution)
- [Invocation contract](#invocation-contract)
- [The run's own checkout](#the-runs-own-checkout)
- [Data flow](#data-flow)
- [Logging responsibility split](#logging-responsibility-split)
- [Durable recovery ownership](#durable-recovery-ownership)
- [What amplihack does NOT do](#what-amplihack-does-not-do)
- [Operational contract](#operational-contract)

---

## Why external

The recipe runner (`recipe-runner-rs`) is a separate Rust binary maintained
in its own repository (`rysweet/amplihack-recipe-runner`). This separation
exists because:

1. **Independent release cadence** — Recipe execution semantics change more
   frequently than CLI behavior.
2. **Build isolation** — The runner has different dependencies (YAML parsing,
   step execution, agent spawning) that would bloat the CLI.
3. **Replaceability** — The CLI treats the runner as a black box behind a
   stable CLI interface.

## Binary resolution

Amplihack resolves the selected runner through the
[shared binary lookup](../reference/recipe-command.md#recipe-runner-binary).
Managed installation and freshness use the immutable upstream revision in
`claude-plugin/recipe-runner.rev`, with the existing ancestry/downgrade
guard. They do not select the remote `main` tip at launch.

Capability validation is separate from managed source provenance. An
explicit compatible custom runner need not match the managed SHA, but it
must satisfy the required provider capabilities. Every provider must also advertise `durable_recovery_v1`. See
[runner validation](../reference/recipe-runner-validation.md).

## Invocation contract

amplihack invokes the runner as a subprocess:

```
amplihack recipe run <recipe-name> \
    --context key1=value1 \
    --context key2=value2
```

The runner:
1. Resolves the recipe YAML from the search path
2. Validates schema and step dependencies
3. Executes steps sequentially, threading context variables between them
4. Streams live progress, heartbeat, and failure diagnostics to stderr
5. Writes the final structured result to stdout
6. Returns exit code 0 on success, 1 on failure

## The run's own checkout

A recipe step is often a full-tool agent. If it ran in your checkout, one
`git add -A` or a pre-commit hook could change your index while you work.
So the CLI, not the runner, owns where steps run.

For a Git working directory, Rust copies `HEAD`, the index and the working
tree separately into a private repository and a detached checkout before the
runner starts. The runner gets `-C <exec dir>`. Every independent launch,
including one nested inside another run, gets its own checkout and index.
Nothing registers a worktree in your repository, and no step writes to your
work tree, index, `HEAD`, refs or configuration. That is why the old
`CallerGitState` repair of `core.bare` (#964) is gone: there is nothing in
the caller's checkout to repair.

Identity and execution are kept apart. Recovery admission keys on your
`--working-dir`, because that is the run you asked for. The `exec dir` belongs
to one attempt: a resumed run prepares a fresh one.

The runner still owns step order and context evaluation. The CLI owns what
surrounds it, in this order: prepare the checkout, start the Linux observer,
launch the runner, then remove the checkout, publish the Git record, finish
the recovery admission and finish the workspace. Each step's error is kept and
joined to the others, so a failed release cannot hide a failed publication.

A dry run plans only. It uses the caller's directory, prepares no checkout,
records nothing and writes no run folder.

Private Git state is not an OS sandbox. See the
[checkout isolation reference](../reference/recipe-checkout-isolation.md)
and the [Git command record](../reference/recipe-runner-logging.md#git-command-record).

---

## Data flow

```
┌─────────────┐     CLI args      ┌──────────────────┐
│ amplihack   │──────────────────▶│ recipe-runner-rs  │
│ (CLI)       │                   │ (external binary) │
└─────────────┘                   └──────────────────┘
                                         │
                              ┌──────────┼──────────┐
                              ▼          ▼          ▼
                         ┌────────┐ ┌────────┐ ┌────────┐
                         │ bash   │ │ agent  │ │ recipe │
                         │ step   │ │ step   │ │ step   │
                         └────────┘ └────────┘ └────────┘
```

Context variables flow forward: each step's `output` key becomes available
to subsequent steps via `{{variable_name}}` interpolation.

## Logging responsibility split

`recipe-runner-rs` owns execution transparency because it is the process that
knows which step, child process, agent, or nested recipe is active.

| Component | Responsibility |
| --- | --- |
| `recipe-runner-rs` | Emit step lifecycle progress, heartbeats, JSONL log events, bounded recent stdout/stderr snippets, and failure context |
| `amplihack` CLI | Resolve and launch the runner, forward stderr progress by default, preserve stdout for final structured output, and preserve additive JSON fields without requiring them |

This split keeps recipe semantics inside the runner while letting the CLI remain
a stable process supervisor. The CLI must not reinterpret step order or child
state. It forwards the runner's stderr progress, captures stdout for the final
`--format` result, and displays optional diagnostic fields when present. When
the runner emits additive diagnostic fields, the CLI must not drop them while
formatting JSON or YAML output.

## Durable recovery ownership

Durable relaunch recording and replay live in the external Rust runner; the
CLI integrates its upstream library contracts and supported executable pin.
See the [recovery reference](../reference/recipe-recovery.md).

Durable recovery uses the same external Rust executor. Amplihack supplies
the canonical repository/work identity, durable run storage, effective
launch settings, and recognized failure causes. Under an identity lock it
selects the latest predecessor, registers the new attempt, and publishes
the latest marker before execution. Only the executing process admits an
attempt; detached launchers do not. Dry runs do not change recovery state.

The runner owns versioned completion records, typed context deltas,
compatibility checks, resource validation, and replay. Its single durable
writer preserves completed children beneath incomplete parents. Trusted
delegated runners attach through inherited private local IPC scoped to
their attempt, invocation subtree, operations, and generation. The CLI
does not reconstruct or reorder steps.

Recognized usage-limit blockage is published before waiting or unwinding.
Synchronized status revisions and generations prevent stale child reports
from overriding a newer enclosing outcome. Current session ownership,
governor admission, recursion accounting, and authorization are established
before dependent replay. Existing same-process wait/retry remains intact.

The selected runner must advertise `durable_recovery_v1`, even for
`--fresh`. Support is delivered upstream before Amplihack's supported
runner pin advances; existing ancestry validation remains required. A
second execution engine or vendored runner copy is not part of recovery.

Private state and required artifact copies live under the run's
`recovery/` directory, outside `runtime/`. Latest-attempt markers survive
retirement of detailed history. Each recovering attempt imports its own
inherited state, allowing repeated recovery without retaining every
ancestor. See [recovery behavior](recipe-usage-limit-recovery.md) and the
[versioned recovery contract](../reference/recipe-recovery.md).

## What amplihack does NOT do

- **Does not parse recipes** — YAML parsing is the runner's responsibility.
- **Does not execute steps** — Step dispatch (bash/agent/recipe) is handled
  by the runner.
- **Does not manage step state** — Context threading, condition evaluation,
  and output capture are runner internals.
- **Does not embed the runner** — There is no compiled-in recipe execution
  engine.

amplihack is responsible for binary resolution, freshness and capability
checks, argument forwarding, and exit code propagation. The recovery
feature adds admission/storage and launch/failure integration to those
responsibilities. General recipe recording and replay will remain runner
responsibilities.

## Operational contract

The goal is a single native recipe runner. Consolidation requires:

1. **Keeping all recipe step types covered** by native execution
2. **Avoiding language-specific recipe shims** in `amplifier-bundle/tools/`
3. **Validating all recipes** against the native runner
4. **Failing loudly** when a recipe depends on an unavailable helper

## Related

- [amplihack recipe](../reference/recipe-command.md) — CLI reference for the `recipe` subcommand
- [Recipe Runner Logging](../reference/recipe-runner-logging.md) — Progress, heartbeat, snippets, and JSON schema
- [Recipe Execution Flow](./recipe-execution-flow.md) — Step-by-step execution semantics
- [Recipe Executor Environment](../reference/recipe-executor-environment.md) — Environment variables for recipe steps
