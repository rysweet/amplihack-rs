# Hive Mind — Evaluation

This page describes the **in-process evaluation harness** that ships in
`amplihack-hive` today. It is the contract that any future cloud-backed
runner must preserve.

> **Rust port note**
> Upstream `amplihack` runs the long-horizon eval as
> `python -m amplihack.eval.long_horizon_memory` against an Azure-deployed
> hive (Container Apps + Service Bus / Event Hubs). The Rust port at
> [`amplihack-hive::hive_eval`](../reference/hive-api.md) keeps the same
> event-loop contract but executes against an in-process [`LocalEventBus`].
> Cloud transport and packaged eval reports are upstream responsibilities
> until the Rust port wires those backends — see
> [Getting Started → Status](../tutorials/hive-mind-getting-started.md#status).

## What Lives In This Repo

- The native eval loop:
  [`amplihack_hive::run_eval`](../reference/hive-api.md) and
  [`amplihack_hive::run_eval_with_responder`](../reference/hive-api.md)
- The companion learning feed:
  [`amplihack_hive::feed::run_feed`](../reference/hive-api.md)
- CLI argument structs and runner functions in
  `crates/amplihack-cli/src/commands/hive_haymaker.rs` (not yet routed as
  `amplihack` subcommands)

## What Does **Not** Live In This Repo

- Long-horizon dataset generation
- Azure Container Apps / Service Bus / Event Hubs deployment assets
- The packaged `run_distributed_eval.sh` wrapper and report bundle layout
- Standard / holdout question slicing

Those continue to live in upstream `rysweet/amplihack` (Python). The Rust
port intentionally focuses on the runtime side.

## Eval Loop Contract

```rust
use amplihack_hive::{HiveEvalConfig, LocalEventBus, run_eval_with_responder};

let cfg = HiveEvalConfig::new(vec![
    "What port does PostgreSQL use?".into(),
    "What is the default Nginx port?".into(),
])
.with_timeout(10)               // seconds to wait for responses per query
.with_min_responses(1);         // minimum responses before moving on

let mut bus = LocalEventBus::new();
let result = run_eval_with_responder(&mut bus, &cfg, |question| {
    // Replace this stub with real agents / SDK calls.
    vec![("agent-1".into(), format!("echo: {question}"), 0.5)]
})?;

assert_eq!(result.total_queries, cfg.questions.len());
```

Topics involved (from `amplihack_hive::hive_events`):

| Topic                  | Direction         | Payload                                  |
| ---------------------- | ----------------- | ---------------------------------------- |
| `HIVE_QUERY`           | harness → agents  | `{ query_id, question }`                 |
| `HIVE_QUERY_RESPONSE`  | agents → harness  | `{ query_id, agent_id, answer, confidence }` |
| `HIVE_LEARN_CONTENT`   | feeder → agents   | `{ feed_id, item }`                      |
| `HIVE_FEED_COMPLETE`   | feeder → agents   | `{ feed_id, items_published }`           |
| `HIVE_AGENT_READY`     | agents → harness  | `{ agent_id }`                           |

`run_eval(bus, &cfg)` is the no-responder form: it publishes queries and
collects whatever subscribers are already on the bus. Use it when wiring
real agents in advance; use `run_eval_with_responder` for self-contained
tests and demos.

## Output

`HiveEvalResult` contains:

- `query_results: Vec<QueryResult>` — one entry per question with all
  collected `AgentAnswer { agent_id, answer, confidence }`
- `total_queries: usize`
- `total_responses: usize`
- `average_confidence: f64`

Render to text with the helper in `hive_haymaker.rs`:

```rust
use amplihack_cli::commands::hive_haymaker::format_eval_text;
println!("{}", format_eval_text(&result));
```

JSON output is via `serde_json::to_string_pretty(&result)?`.

## Library API: Feed + Eval In One Process

```rust
use amplihack_hive::{
    FeedConfig, HiveEvalConfig, LocalEventBus, feed::run_feed,
    run_eval_with_responder,
};

let mut bus = LocalEventBus::new();

// 1. Seed content into the hive.
let feed = run_feed(
    &mut bus,
    &FeedConfig::new("smoke", vec!["Photosynthesis happens in chloroplasts.".into()]),
)?;
assert_eq!(feed.items_published, 1);

// 2. Run questions against agents. (Stub responder for illustration.)
let cfg = HiveEvalConfig::new(vec!["Where does photosynthesis happen?".into()]);
let result = run_eval_with_responder(&mut bus, &cfg, |q| {
    vec![("a1".into(), format!("answer to {q}"), 0.8)]
})?;
assert_eq!(result.total_queries, 1);
```

## CLI Surface (Planned — Not Yet Wired)

`hive_haymaker.rs` defines `HiveFeedArgs` and `HiveEvalArgs`. Today these
are exercised via unit tests and direct library calls, **not** via
`amplihack hive feed` or `amplihack hive eval` — those subcommands are not
registered in `cli_subcommands.rs` yet. A tracking issue should land before
the docs claim a CLI invocation form.

The argument shape that *will* be exposed once wired:

| Struct          | Field            | Default       | Notes                                            |
| --------------- | ---------------- | ------------- | ------------------------------------------------ |
| `HiveFeedArgs`  | `deployment_id`  | (required)    | Logical hive identifier                          |
|                 | `turns`          | `100`         | Number of `HIVE_LEARN_CONTENT` events            |
|                 | `topic`          | env / default | Override Service Bus topic name                  |
|                 | `sb_conn_str`    | `""`          | **Stub** — logs a warning and uses `LocalEventBus` |
| `HiveEvalArgs`  | `deployment_id`  | (required)    | Logical hive identifier                          |
|                 | `repeats`        | `3`           | Question rounds                                  |
|                 | `wait_for_ready` | `0`           | Stubbed — never blocks                           |
|                 | `timeout`        | `600`         | Per-round timeout (seconds)                      |
|                 | `topic`          | env / default | Override Service Bus topic name                  |
|                 | `sb_conn_str`    | `""`          | **Stub** — logs a warning and uses `LocalEventBus` |
|                 | `output`         | `Text`        | `Text` or `Json`                                 |

There are no `--mode`, `--question-set`, `--agents`, `--seeds`,
`--memory-type`, `--sdk`, `--answer-mode`, `--parallel-workers`, `--load-db`,
or `--skip-learning` flags in this port today. Those concepts live upstream.

## Cloud Transport (Planned — Stubbed Today)

Both runners check `sb_conn_str`; if non-empty the runner logs:

```
Service Bus transport is not yet implemented in Rust; using local event bus
```

…and continues against a fresh `LocalEventBus`. There is no silent
fall-through to noop and no Event Hubs path. The cloud transport story will
land alongside the wired CLI subcommand.

## Related Docs

- [Hive Mind Getting Started](../tutorials/hive-mind-getting-started.md)
- [Hive Mind Tutorial](../tutorials/hive-mind-tutorial.md)
- [Hive Mind Design](./hive-mind-design.md)
- [`amplihack-hive` API reference](../reference/hive-api.md)
- [LadybugDB reference](../reference/ladybug-reference.md)
