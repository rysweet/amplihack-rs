# Distributed Hive Eval — Getting Started

This page is the fastest way to get from a clean checkout to a real distributed
eval running against the [`amplihack-hive`](../reference/hive-api.md) crate.

> **Rust port note**
> Upstream `amplihack` ships the hive eval as Python wrappers
> (`python -m amplihack.eval.long_horizon_memory`). In `amplihack-rs` the same
> functionality is reached through the native `amplihack hive ...` subcommands
> (wired through `crates/amplihack-cli/src/commands/hive_haymaker.rs`) backed
> by the [`amplihack-hive`](../reference/hive-api.md) crate. The local
> code-graph each agent owns is stored in [`lbug`](../reference/ladybug-reference.md),
> the native Rust embedded graph database (formerly `kuzu`).

## Prerequisites

You need a checkout of `amplihack-rs` plus, for distributed Azure runs, the
sibling `amplihack-agent-eval` repository:

- `amplihack-rs` — agent runtime, native CLI, and Azure deployment assets
- `amplihack-agent-eval` — dataset generation, distributed runner, and reports

You also need:

- Azure CLI authenticated to the target subscription
- A working Rust toolchain (`cargo --version`) for building `amplihack`
- `ANTHROPIC_API_KEY` set for grading and the default learning-agent runtime

## Local Smoke Run From This Repo

Use the thin local subcommand when you are editing `amplihack-rs` and want a
fast local check.

```bash
cd /path/to/amplihack-rs

amplihack hive eval \
  --turns 100 \
  --questions 20 \
  --question-set standard \
  --output-dir /tmp/eval-run
```

Behind the scenes this calls into
[`amplihack_hive::hive_eval`](../reference/hive-api.md) using the in-process
`LocalEventBus`, so it does not require any cloud resources.

## Real Azure Distributed Run

Switch to the eval repo for the end-to-end wrapper.

```bash
cd /path/to/amplihack-agent-eval

export ANTHROPIC_API_KEY=...
export AMPLIHACK_SOURCE_ROOT=/path/to/amplihack-rs

./run_distributed_eval.sh \
  --agents 100 \
  --turns 5000 \
  --questions 50 \
  --question-set standard
```

Reuse an existing deployment instead of redeploying:

```bash
SKIP_DEPLOY=1 \
HIVE_NAME=amplihive3175e \
HIVE_RESOURCE_GROUP=hive-pr3175-rg \
./run_distributed_eval.sh \
  --agents 100 \
  --turns 5000 \
  --questions 50 \
  --question-set holdout
```

## What To Expect

- the wrapper deploys or refreshes Azure Container Apps from
  `deploy/azure_hive/`
- agent traffic uses Event Hubs, not Service Bus
- the result bundle includes the final report JSON, logs, and rerun metadata

## Standard vs Holdout

Use `--question-set standard` for the canonical slice and
`--question-set holdout` for a deterministic alternate slice.

That makes it possible to re-evaluate the same runtime against a different
question subset without changing the fact-generation path.

## Where To Read Next

- [Distributed Hive Evaluation](../concepts/hive-mind-eval.md)
- [Hive Mind Tutorial](./hive-mind-tutorial.md)
- [Hive Mind Design](../concepts/hive-mind-design.md)
- [`amplihack-hive` API reference](../reference/hive-api.md)
- [LadybugDB reference](../reference/ladybug-reference.md)
