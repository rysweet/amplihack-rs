# Distributed Hive Evaluation

`amplihack-rs` owns the **agent/runtime side** of the distributed eval story.

- `deploy/azure_hive/` contains the Azure Container Apps and Event Hubs
  deployment assets
- [`amplihack-hive::hive_eval`](../reference/hive-api.md) is the native Rust
  evaluation harness used from this repo
- The local long-horizon and multi-seed wrappers are exposed through the
  `amplihack hive eval` subcommand
  (`crates/amplihack-cli/src/commands/hive_haymaker.rs`)

The authoritative long-horizon dataset generator and Azure distributed
runner live in the sibling `amplihack-agent-eval` repo.

> **Rust port note**
> Upstream `amplihack` invokes the long-horizon eval as
> `python -m amplihack.eval.long_horizon_memory`. In `amplihack-rs` the same
> behavior is exposed via `amplihack hive eval ...`, backed by the
> [`amplihack-hive`](../reference/hive-api.md) crate and persisted through
> the [`lbug`](../reference/ladybug-reference.md) graph backend.

## Read This Next

- [Hive Mind Getting Started](../tutorials/hive-mind-getting-started.md) —
  fastest path from a clean checkout to a real distributed eval
- [Hive Mind Tutorial](../tutorials/hive-mind-tutorial.md) — full walkthrough
  of the federated hive mind APIs
- [`amplihack-hive` API reference](../reference/hive-api.md) — the Rust
  surface that powers `amplihack hive`

## Use This Repo For

- changing the agent runtime
- changing the Azure deployment shape
- running the thin local wrappers while you are editing `amplihack-rs`

## Use `amplihack-agent-eval` For

- authoritative long-horizon question generation
- the Event Hubs distributed runner
- packaged eval reports and rerun metadata
- the end-to-end `run_distributed_eval.sh` wrapper

## Local Wrapper: Single Run

The local wrapper delegates to the
[`amplihack-hive`](../reference/hive-api.md) crate's `hive_eval` module:

```bash
amplihack hive eval \
  --turns 100 \
  --questions 20 \
  --output-dir /tmp/eval-run
```

Useful flags from this wrapper:

- `--sdk {mini,claude,copilot,microsoft}`
- `--memory-type {auto,hierarchical,cognitive}`
- `--answer-mode {single-shot,agentic}`
- `--parallel-workers <N>`
- `--load-db` and `--skip-learning`

If you need `standard` versus `holdout` question slices, use the
`amplihack-agent-eval` CLI or distributed runner rather than the thin
wrappers in this repo.

## Local Wrapper: Multi-Seed Comparison

```bash
amplihack hive eval \
  --mode multi-seed \
  --turns 100 \
  --questions 20 \
  --seeds 42,123,456,789 \
  --output-dir /tmp/eval-compare
```

## Distributed Azure Run

For real Azure distributed runs, switch to the sibling
`amplihack-agent-eval` repo and use its wrapper or direct runner.

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

That path drives the Azure deployment assets from this repo, but the
harness and reporting stay centralized in `amplihack-agent-eval`.

## Question Sets

| Value      | Meaning                                                       |
| ---------- | ------------------------------------------------------------- |
| `standard` | Canonical deterministic question slice                        |
| `holdout`  | Alternate deterministic slice for anti-overfitting validation |

`holdout` changes which questions are asked. It does not generate a second
fact universe.

## Important Checkout Note

`amplihack-rs` is a Cargo workspace; the eval entry points are shipped as
part of the `amplihack` binary. As long as you launch through
`cargo run -p amplihack -- hive eval ...` (or an installed `amplihack`
binary), there is no `PYTHONPATH` to manage. When validating local changes
to a sibling `amplihack-agent-eval` checkout, use its own runner; do not
mix Python wrappers into the Rust evaluation path.

## Current Verified Results

A current validation snapshot, including the accepted Azure scores and the
latest reproducible local test commands, lives upstream at
`docs/hive_mind/current-validation-results.md` in `rysweet/amplihack`.

## Related Docs

- [Hive Mind Getting Started](../tutorials/hive-mind-getting-started.md)
- [Hive Mind Tutorial](../tutorials/hive-mind-tutorial.md)
- [Hive Mind Design](./hive-mind-design.md)
- [`amplihack-hive` API reference](../reference/hive-api.md)
- [LadybugDB reference](../reference/ladybug-reference.md)
- `amplihack-agent-eval/docs/distributed-hive-eval.md`
- `amplihack-agent-eval/docs/running-evals.md`
