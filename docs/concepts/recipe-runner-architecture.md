# Recipe Runner Architecture

Why the recipe runner is an external binary, how amplihack locates and invokes
it, and how recipe execution stays predictable.

## Contents

- [Why external](#why-external)
- [Binary resolution](#binary-resolution)
- [Invocation contract](#invocation-contract)
- [Data flow](#data-flow)
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

amplihack resolves the runner binary at launch time using `freshness.rs`:

```
$PATH lookup for `recipe-runner-rs`
       │
       ├── found → check freshness against GitHub HEAD
       │              │
       │              ├── up-to-date → use it
       │              └── stale → `cargo install --git` to upgrade
       │
       └── not found → `cargo install --git` to install
```

The freshness check compares the locally installed commit SHA against the
remote `main` branch HEAD, throttled by a cooldown file at
`~/.amplihack/state/recipe_runner.json`.

Source: `crates/amplihack-cli/src/freshness.rs`, lines 108–176.

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
4. Returns exit code 0 on success, 1 on failure

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

## What amplihack does NOT do

- **Does not parse recipes** — YAML parsing is the runner's responsibility.
- **Does not execute steps** — Step dispatch (bash/agent/recipe) is handled
  by the runner.
- **Does not manage step state** — Context threading, condition evaluation,
  and output capture are runner internals.
- **Does not embed the runner** — There is no compiled-in recipe execution
  engine.

amplihack is responsible for: binary resolution, freshness checks, argument
forwarding, and exit code propagation.

## Operational contract

The goal is a single native recipe runner. Consolidation requires:

1. **Keeping all recipe step types covered** by native execution
2. **Avoiding language-specific recipe shims** in `amplifier-bundle/tools/`
3. **Validating all recipes** against the native runner
4. **Failing loudly** when a recipe depends on an unavailable helper

## Related

- [amplihack recipe](../reference/recipe-command.md) — CLI reference for the `recipe` subcommand
- [Recipe Execution Flow](./recipe-execution-flow.md) — Step-by-step execution semantics
- [Recipe Executor Environment](../reference/recipe-executor-environment.md) — Environment variables for recipe steps
