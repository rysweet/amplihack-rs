# Recipe Runner Architecture

Why the recipe runner is an external binary, how amplihack-rs locates and
invokes it, and what the consolidation plan means for the codebase.

## Contents

- [Why external](#why-external)
- [Binary resolution](#binary-resolution)
- [Invocation contract](#invocation-contract)
- [Data flow](#data-flow)
- [What amplihack-rs does NOT do](#what-amplihack-rs-does-not-do)
- [The Python runner: why it still exists](#the-python-runner-why-it-still-exists)
- [Consolidation direction](#consolidation-direction)

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

amplihack-rs resolves the runner binary at launch time using `freshness.rs`:

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

amplihack-rs invokes the runner as a subprocess:

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

## What amplihack-rs does NOT do

- **Does not parse recipes** — YAML parsing is the runner's responsibility.
- **Does not execute steps** — Step dispatch (bash/agent/recipe) is handled
  by the runner.
- **Does not manage step state** — Context threading, condition evaluation,
  and output capture are runner internals.
- **Does not embed the runner** — There is no compiled-in recipe execution
  engine.

amplihack-rs is responsible for: binary resolution, freshness checks,
argument forwarding, and exit code propagation.

## The Python runner: why it still exists

A Python-based recipe runner (`amplifier-bundle/tools/recipe_runner.py`)
predates the Rust implementation. Both runners coexist because:

- **Legacy recipes** may depend on Python-specific behavior not yet ported.
- **The `amplifier-bundle`** still ships Python utilities that some recipes
  reference.
- **Migration is incomplete** — Not all step types have Rust equivalents.

The Rust runner is the default for new recipes. The Python runner is a
fallback, not a parallel production system.

## Consolidation direction

The goal is a single Rust recipe runner. Consolidation requires:

1. **Porting remaining step types** from Python to Rust
2. **Removing Python-specific recipe shims** in `amplifier-bundle/tools/`
3. **Validating all recipes** against the Rust runner exclusively
4. **Deleting the Python runner** once no recipe depends on it

See [amplihack Retirement Direction](./amplihack-retirement-direction.md)
for the broader Python winddown timeline.

## Related

- [amplihack recipe](../reference/recipe-command.md) — CLI reference for the `recipe` subcommand
- [Recipe Execution Flow](./recipe-execution-flow.md) — Step-by-step execution semantics
- [Recipe Executor Environment](../reference/recipe-executor-environment.md) — Environment variables for recipe steps
