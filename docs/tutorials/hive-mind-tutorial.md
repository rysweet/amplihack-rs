# Hive Mind — Tutorial

Walk through the [`amplihack-hive`](../reference/hive-api.md) library API end
to end: storing facts, gossiping between peers, running the eval harness, and
driving the distributed primitives. Every snippet below uses symbols that are
re-exported from `amplihack_hive::*` today.

> **Rust port note**
> Upstream `amplihack` (Python) exposes the hive mind through
> `from amplihack.agents.goal_seeking.hive_mind …`. The Rust port keeps the
> same conceptual layers (storage, transport, discovery, query) but with a
> trimmed public surface — see
> [`hive-api.md`](../reference/hive-api.md) for the authoritative list. The
> CLI bridge (`amplihack hive …`) and Azure deployment surface are planned
> but **not wired** yet. See the [Getting Started](./hive-mind-getting-started.md#status)
> page for the current status.

## Prerequisites

```bash
cd /path/to/amplihack-rs
cargo build -p amplihack-hive
```

## 1. Single-Process Hive (`HiveMindOrchestrator`)

`HiveMindOrchestrator` wraps a [`HiveGraph`], a [`LocalEventBus`], an optional
[`GossipProtocol`], and a [`PromotionPolicy`]. It is the "hello world"
entry point.

```rust
use amplihack_hive::{HiveMindOrchestrator, Result};

fn main() -> Result<()> {
    let mut hive = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("alice".to_string());

    let outcome = hive.store_and_promote(
        "networking",
        "PostgreSQL listens on TCP 5432",
        0.95,
        "alice",
    )?;
    assert!(outcome.promoted);

    for fact in hive.query("networking")? {
        println!("  [{:.0}%] {}", fact.confidence * 100.0, fact.content);
    }
    Ok(())
}
```

The default [`DefaultPromotionPolicy`] thresholds are
`promote=0.7 / broadcast=0.9 / gossip=0.5`. Pass a custom `Box<dyn
PromotionPolicy>` to [`HiveMindOrchestrator::new`] to override.

## 2. Multi-Peer Federation Through Orchestrators

Federation in `amplihack-hive` today is composed by attaching peer
orchestrators. `query_unified` aggregates and de-duplicates across the local
graph and every attached peer.

```rust
use amplihack_hive::{HiveMindOrchestrator, Result};

fn main() -> Result<()> {
    let mut security = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("sec-1".to_string());
    security.store_fact("vulnerabilities", "CVE-2024-1234 hits OpenSSL 3.x", 0.95, "sec-1")?;

    let mut infra = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("infra-1".to_string());
    infra.store_fact("servers", "prod-db-01 = 10.0.1.5:5432", 0.90, "infra-1")?;

    let mut root = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("root".to_string());
    root.add_peer(security);
    root.add_peer(infra);

    let merged = root.query_unified("vulnerabilities")?;
    println!("federated hits: {}", merged.len());
    Ok(())
}
```

There is **no** tree-shaped federation API (no `set_parent` / `add_child` on
`HiveGraph`) in the public re-exports today. If you need a hierarchy, compose
peer orchestrators recursively or use the lower-level
`crate::graph::federation` helpers.

## 3. Distributed Primitives (`AgentNode` + `HiveCoordinator`)

`AgentNode` is the per-agent unit; `HiveCoordinator` tracks domain experts,
trust scores, and contradictions. They are wired together by the test
harnesses in `crates/amplihack-hive/src/distributed/`.

```rust
use std::sync::{Arc, Mutex};
use amplihack_hive::distributed::{AgentNode, HiveCoordinator};
use amplihack_hive::LocalEventBus;

fn main() -> amplihack_hive::Result<()> {
    let bus = Arc::new(Mutex::new(LocalEventBus::new()));
    let coordinator = Arc::new(Mutex::new(HiveCoordinator::new()));
    coordinator.lock().unwrap().register_agent("bio-agent", "biology");

    let mut agent = AgentNode::new("bio-agent", "biology");
    agent.join_hive(bus.clone(), coordinator.clone());
    agent.learn(
        "biology",
        "Mitosis is the process of nuclear cell division",
        0.9,
        Some(vec!["intro".into()]),
    )?;

    let hits = agent.query("cell division", 10);
    println!("local hits: {}", hits.len());
    Ok(())
}
```

`AgentNode::join_hive(bus, coordinator)` is the only constructor that wires
in the bus and coordinator; there is **no** builder API and **no**
`hive_store` parameter today.

## 4. Eval Harness (`run_eval_with_responder`)

`amplihack_hive::run_eval_with_responder` drives a `HIVE_QUERY` /
`HIVE_QUERY_RESPONSE` event loop on a `LocalEventBus`. The closure represents
the population of agents that would normally subscribe.

```rust
use amplihack_hive::{HiveEvalConfig, LocalEventBus, run_eval_with_responder};

fn main() -> amplihack_hive::Result<()> {
    let cfg = HiveEvalConfig::new(vec![
        "What port does PostgreSQL use?".into(),
        "What is the default Nginx port?".into(),
    ])
    .with_timeout(5)
    .with_min_responses(1);

    let mut bus = LocalEventBus::new();
    let result = run_eval_with_responder(&mut bus, &cfg, |question| {
        // Two stubbed agents for illustration.
        vec![
            ("agent-a".into(), format!("a says: {question}"), 0.7),
            ("agent-b".into(), format!("b says: {question}"), 0.4),
        ]
    })?;

    for qr in &result.query_results {
        println!("Q: {}", qr.question);
        for ans in &qr.answers {
            println!("  [{}] {} (conf={:.2})", ans.agent_id, ans.answer, ans.confidence);
        }
    }
    Ok(())
}
```

The bare `run_eval(bus, &cfg)` form runs without a responder and will
report `0` responses against an unsubscribed bus — use it only when wiring
your own subscribers in advance.

## 5. Learning Feed (`run_feed`)

The companion to `run_eval` is `run_feed`, which publishes
`HIVE_LEARN_CONTENT` events followed by a `HIVE_FEED_COMPLETE` sentinel.

```rust
use amplihack_hive::{LocalEventBus, FeedConfig, feed::run_feed};

fn main() -> amplihack_hive::Result<()> {
    let items = vec![
        "Photosynthesis happens in chloroplasts.".to_string(),
        "Water has a 104.5° bond angle.".to_string(),
    ];
    let cfg = FeedConfig::new("smoke-deploy", items);

    let mut bus = LocalEventBus::new();
    let result = run_feed(&mut bus, &cfg)?;
    println!("published={} errors={}", result.items_published, result.errors.len());
    Ok(())
}
```

## 6. CLI Surface (Planned — Not Yet Wired)

`crates/amplihack-cli/src/commands/hive_haymaker.rs` defines two arg structs
plus runner functions:

```text
HiveFeedArgs { deployment_id, turns, topic, sb_conn_str }
HiveEvalArgs { deployment_id, repeats, wait_for_ready, timeout, topic, sb_conn_str, output }
fn run_hive_feed(&HiveFeedArgs) -> Result<FeedResult>
fn run_hive_eval(&HiveEvalArgs) -> Result<HiveEvalResult>
```

These functions are exercised by unit tests but are **not yet exposed as
`amplihack` subcommands** — `cli_subcommands.rs` contains zero `hive`
references at the time of writing. To use them, call `run_hive_feed` /
`run_hive_eval` directly from Rust as a library API. Wiring them as real
CLI subcommands is tracked as planned work.

## 7. Cloud Transport (Planned — Stubbed Today)

`HiveFeedArgs::sb_conn_str` and `HiveEvalArgs::sb_conn_str` exist for
forward-compatibility but are not consumed. The runner logs:

```
Service Bus transport is not yet implemented in Rust; using local event bus
```

…and falls back to a fresh `LocalEventBus`. There is no Event Hubs path
either. Treat any cloud-mode discussion as design intent until those
transports land.

## Architecture Snapshot

```
HiveMindOrchestrator (per agent)
  ├── HiveGraph              (storage)
  ├── LocalEventBus          (transport, in-process)
  ├── GossipProtocol         (optional, peer discovery)
  ├── PromotionPolicy        (DefaultPromotionPolicy)
  └── peers: Vec<HiveMindOrchestrator>   (federation by composition)

distributed::HiveCoordinator   ← shared expert registry & trust ledger
distributed::AgentNode         ← per-agent unit, joins via Arc<Mutex<…>>
hive_eval::run_eval_*          ← event-loop harness
feed::run_feed                 ← event-loop content publisher
controller::HiveController     ← desired-state YAML manifests (HiveManifest)
```

## Key Modules

| Module                                               | Public surface                                                  |
| ---------------------------------------------------- | --------------------------------------------------------------- |
| `crates/amplihack-hive/src/orchestrator.rs`          | `HiveMindOrchestrator`, `DefaultPromotionPolicy`, `PromotionResult` |
| `crates/amplihack-hive/src/graph/`                   | `HiveGraph` (struct), `tokenize`, `word_overlap`, search helpers |
| `crates/amplihack-hive/src/distributed/`             | `AgentNode`, `HiveCoordinator`                                  |
| `crates/amplihack-hive/src/event_bus.rs`             | `EventBus` trait, `LocalEventBus` (only impl re-exported)       |
| `crates/amplihack-hive/src/{feed,hive_eval}.rs`      | `run_feed`, `run_eval`, `run_eval_with_responder`               |
| `crates/amplihack-hive/src/controller.rs`            | `HiveController`, `InMemoryGraphStore`, `InMemoryGateway`       |
| `crates/amplihack-hive/src/{bloom,crdt,dht,gossip}.rs` | `BloomFilter`, CRDTs, `DHTRouter`, `GossipProtocol`           |
| `crates/amplihack-cli/src/commands/hive_haymaker.rs` | `HiveFeedArgs`, `HiveEvalArgs`, `run_hive_feed`, `run_hive_eval` (not yet routed) |

## Related

- [Hive Mind Getting Started](./hive-mind-getting-started.md)
- [Hive Mind Design](../concepts/hive-mind-design.md)
- [Hive Mind Eval](../concepts/hive-mind-eval.md)
- [`amplihack-hive` API reference](../reference/hive-api.md)
- [LadybugDB reference](../reference/ladybug-reference.md)
