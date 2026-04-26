# Hive Mind — Getting Started

Run a hive-mind smoke check against the actual `amplihack-hive` crate. This
page targets the **library API that ships today**. The `amplihack hive …`
CLI surface and the Azure deployment described in upstream `amplihack` are
**planned but not yet wired** — see [Status](#status) below.

> **Rust port note**
> Upstream `amplihack` (Python) ships the hive mind as
> `from amplihack.agents.goal_seeking.hive_mind …` and a `python -m
> amplihack.eval.long_horizon_memory` runner backed by Azure Event
> Hubs / Service Bus. The Rust port at
> [`crates/amplihack-hive/`](../reference/hive-api.md) re-implements the
> in-process layers natively. Cloud transport is intentionally stubbed in
> this port — `crates/amplihack-cli/src/commands/hive_haymaker.rs`
> defaults to an in-memory [`LocalEventBus`] and logs a warning if a
> Service Bus connection string is supplied.

## Prerequisites

- A working Rust toolchain (`cargo --version`)
- This repo checked out (no sibling repos required for the smoke run)

## Build

```bash
cd /path/to/amplihack-rs
cargo build -p amplihack-hive
```

## 30-Second Smoke Run (Library API)

Drop this into `examples/hive_smoke.rs` (or any binary in your workspace):

```rust
use amplihack_hive::{HiveMindOrchestrator, Result};

fn main() -> Result<()> {
    let mut hive = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("smoke-agent".to_string());

    // Store a fact and let the default promotion policy decide.
    let outcome = hive.store_and_promote(
        "networking",
        "PostgreSQL listens on TCP 5432",
        0.95,
        "smoke-agent",
    )?;

    println!(
        "stored fact_id={} promoted={} broadcast={}",
        outcome.fact_id, outcome.promoted, outcome.broadcast
    );

    for fact in hive.query("networking")? {
        println!("  [{:.0}%] {}", fact.confidence * 100.0, fact.content);
    }
    Ok(())
}
```

That confirms storage, promotion, and query — no event bus, no peers, no
network.

## Run the In-Process Eval Harness

`amplihack-hive` ships a real eval harness that drives a `LocalEventBus`
end-to-end. Use it from Rust to validate the request/response loop:

```rust
use amplihack_hive::{
    HiveEvalConfig, LocalEventBus, run_eval_with_responder,
};

fn main() -> amplihack_hive::Result<()> {
    let questions = vec![
        "What port does PostgreSQL use?".to_string(),
        "What is the default Nginx port?".to_string(),
    ];
    let config = HiveEvalConfig::new(questions).with_timeout(5);

    let mut bus = LocalEventBus::new();

    // The responder closure decides how each query is answered.
    let result = run_eval_with_responder(&mut bus, &config, |q| {
        vec![(
            "smoke-agent".to_string(),
            format!("echo: {q}"),
            0.5,
        )]
    })?;

    println!(
        "queries={} responses={} avg_conf={:.2}",
        result.total_queries, result.total_responses, result.average_confidence
    );
    Ok(())
}
```

## Status

What works **today** in `amplihack-rs`:

| Capability                                                 | Where                                                  |
| ---------------------------------------------------------- | ------------------------------------------------------ |
| In-process hive (`HiveMindOrchestrator`, `HiveGraph`)      | `crates/amplihack-hive/src/{orchestrator,graph}`       |
| Local event bus + eval harness                             | `crates/amplihack-hive/src/{event_bus,hive_eval,feed}` |
| Distributed primitives (`AgentNode`, `HiveCoordinator`)    | `crates/amplihack-hive/src/distributed/`               |
| Bloom filters, CRDTs, gossip, DHT, embeddings              | re-exports from `crates/amplihack-hive/src/lib.rs`     |
| `hive feed` / `hive eval` argument types and runners       | `crates/amplihack-cli/src/commands/hive_haymaker.rs`   |

What is **not yet wired** (planned — track via repo issues):

- `amplihack hive {feed,eval,deploy,status,teardown}` is **not** a registered
  subcommand of the `amplihack` binary today. `hive_haymaker.rs` defines the
  arg structs and runner functions, but `cli_subcommands.rs` does not yet
  route to them.
- Azure Service Bus / Event Hubs transport is **stubbed**. Supplying
  `sb_conn_str` logs a warning and falls back to `LocalEventBus`.
- `deploy/azure_hive/` does not exist in this repo. Azure Container Apps
  provisioning lives upstream in `rysweet/amplihack` (Python).

## Where To Read Next

- [Hive Mind Tutorial](./hive-mind-tutorial.md) — full library walkthrough
- [Hive Mind Design](../concepts/hive-mind-design.md) — layered architecture
- [Hive Mind Eval](../concepts/hive-mind-eval.md) — eval harness contract
- [`amplihack-hive` API reference](../reference/hive-api.md)
- [LadybugDB reference](../reference/ladybug-reference.md) — embedded graph DB
