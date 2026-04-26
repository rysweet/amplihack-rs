# Drive a Hive from a Prompt

> **Status: Mixed.** The `feed` and `eval` library APIs in `amplihack-hive`
> are shipped today and operate against a `LocalEventBus`. A top-level
> `amplihack hive …` CLI subcommand is **not yet wired**; the snippets
> labelled "(planned CLI)" below describe the proposed surface. Cross-check
> `crates/amplihack-cli/src/cli_commands.rs` to see the current top-level
> command set.

This how-to shows how to push a learning prompt into a hive and collect
answers, using only what is actually available in the current binary.

## Before you start

You need:

- amplihack-rs built (`cargo build --release`)
- A Rust project that can depend on the `amplihack-hive` crate (for the
  shipped library API), **or**
- Patience to wait for the planned CLI surface (issue tracker link to be
  added once filed)

## Option A — programmatic (works today)

The shipped `run_hive_feed` and `run_hive_eval` functions accept a
`LocalEventBus` and run end-to-end without external infrastructure:

```rust
use amplihack_cli::commands::hive_haymaker::{
    HiveFeedArgs, HiveEvalArgs, run_hive_feed, run_hive_eval, format_eval_text,
};

let feed_args = HiveFeedArgs {
    deployment_id: "my-hive".into(),
    turns: 25,
    topic: None,
    sb_conn_str: String::new(),
};
run_hive_feed(&feed_args)?;

let eval_args = HiveEvalArgs {
    deployment_id: "my-hive".into(),
    repeats: 3,
    wait_for_ready: 0,
    timeout: 600,
    topic: None,
    sb_conn_str: String::new(),
    output: Default::default(),
};
let result = run_hive_eval(&eval_args)?;
println!("{}", format_eval_text(&result));
```

The handler logs `info!` and `warn!` events and returns a typed
`HiveEvalResult`. With an empty `sb_conn_str` it operates entirely
in-process against `LocalEventBus`.

## Option B — CLI (planned)

A future release intends to expose the same handlers as a top-level
subcommand:

```text
# Planned — not implemented yet
amplihack hive feed --deployment-id my-hive --turns 25
amplihack hive eval --deployment-id my-hive --repeats 3 --output json
```

When this lands, this page will be updated with concrete invocations.
Until then, prefer Option A.

## Verify the run

The `HiveEvalResult` returned from Option A contains the per-round
question/answer breakdown and an aggregate confidence score. There is no
shipped `hive status` command; inspect the returned struct or, in tests,
use the `LocalEventBus` accessor directly.

## See also

- [Hive Orchestration](../concepts/hive-orchestration.md)
- [Hive Mind: Distributed Topology](../concepts/hive-mind-distributed.md)
  (planned multi-host design)
- [Deploy a Hive Swarm](./deploy-hive-swarm.md) (Azure deployment walkthrough)
- [Hive API reference](../reference/hive-api.md)
