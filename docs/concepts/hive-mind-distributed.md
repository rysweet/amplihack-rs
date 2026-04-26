# Hive Mind: Distributed Topology

> **Status: Planned (specification).** The shipped hive runtime today is the
> haymaker described in [Hive Orchestration](./hive-orchestration.md). This
> page describes the **planned** distributed topology that builds on top of
> it. CLI surface, event names, and configuration shown here are proposals
> for an upcoming release; cross-check `crates/amplihack-hive/` and
> `crates/amplihack-cli/src/commands/` for what is actually wired today.

This page extends [Hive Orchestration](./hive-orchestration.md) and the
[Hive API reference](../reference/hive-api.md) with the design for running
hives across multiple containers and (eventually) multiple hosts.

## Scope of this page

What it covers:

- The proposed multi-host topology layered on the existing
  `LocalEventBus` / `HiveMindWorkload` / Service Bus design.
- Open design questions (queen recursion, dispatcher leases, clock skew)
  that are **not yet implemented** and need design before code.

What it does **not** cover:

- The shipped `feed` / `eval` flow against a single bus — see
  [Hive Orchestration](./hive-orchestration.md).
- Azure Container Apps deployment specifics — see
  [Deploy a Hive Swarm](../howto/deploy-hive-swarm.md).

## Today vs. Planned

| Capability                                | Today                                | Planned                                  |
|-------------------------------------------|--------------------------------------|------------------------------------------|
| Single-process bus (`LocalEventBus`)      | ✅ shipped                           | unchanged                                 |
| Azure Service Bus transport               | ✅ described in `HiveConfig`          | (no design change planned)               |
| `HiveMindWorkload` deploy lifecycle       | ✅ library API                        | (no change)                              |
| Multi-host swarm coordination             | ❌ not implemented                    | proposed                                  |
| Recursive "queen-of-queens" decomposition | ❌ not implemented                    | proposed (see Open Questions below)      |
| Heartbeat / lease protocol                | ❌ not implemented                    | proposed                                   |

## Proposed multi-host model

A hive deployment is one logical workload spread across N containers, each
hosting M agents (per `HiveConfig::num_containers` × `agents_per_container`).
The proposed extension introduces:

- A **dispatcher** role that owns work distribution decisions for the
  deployment.
- **Worker** containers that run agents and publish results to the existing
  event topics (`hive.learn_content`, `hive.query_response`, etc.).

The dispatcher election protocol, lease durations, heartbeat cadence, and
clock-skew tolerance are **open design questions**; do not assume any
specific values until the design lands.

## Open questions

These items must be resolved before implementation begins:

1. **Election:** Service Bus does not provide leader election. Should we use
   blob-lease-based election, an Azure-managed mechanism, or skip election
   and run all dispatchers idempotently?
2. **Recursion bounds:** If queens can spawn sub-queens, what stops runaway
   fan-out? `AMPLIHACK_MAX_DEPTH` exists for the recipe runner — should the
   hive use the same env var?
3. **Failure semantics:** If a dispatcher dies mid-batch, should pending
   work be re-emitted, dropped, or moved to a dead-letter queue?
4. **Observability:** Today only `info!` / `warn!` tracing is emitted. For
   distributed coordination we need structured events; what is the
   superset of `HiveEvent` topics required?

## See also

- [Hive Orchestration](./hive-orchestration.md) — the shipped concept doc.
- [Deploy a Hive Swarm](../howto/deploy-hive-swarm.md) — single-deployment
  walkthrough that exists today.
- [Hive API reference](../reference/hive-api.md) — typed bindings for the
  current crate surface.
