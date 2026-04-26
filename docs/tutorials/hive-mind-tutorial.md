# Distributed Hive Mind — Tutorial

Get multiple goal-seeking agents sharing knowledge through a federated hive
mind, implemented natively on top of the
[`amplihack-hive`](../reference/hive-api.md) crate and the
[`lbug`](../reference/ladybug-reference.md) graph backend.

> **Rust port note**
> Upstream `amplihack` exposes the hive mind through Python imports
> (`from amplihack.agents.goal_seeking.hive_mind...`). The Rust port at
> `crates/amplihack-hive/` exposes equivalent types as a normal Rust library
> and wires user-facing entry points through the `amplihack` CLI. Examples
> below show the Rust API; see [`hive-api.md`](../reference/hive-api.md) for
> the full surface.

## Prerequisites

```bash
# From the amplihack-rs repo root
cargo build --release -p amplihack
```

## 1. Local Quick Start (In-Memory, Single Process)

The fastest way to see the hive mind work. All state lives in process memory.

```rust
use amplihack_hive::graph::{InMemoryHiveGraph, HiveFact, create_hive_graph};

// Create a hive
let mut hive = create_hive_graph("memory", "my-hive");

// Register agents
hive.register_agent("alice", "security");
hive.register_agent("bob", "infrastructure");

// Alice promotes a fact
hive.promote_fact("alice", HiveFact {
    fact_id: String::new(),
    content: "SSH runs on port 22".into(),
    concept: "networking".into(),
    confidence: 0.95,
});

// Bob promotes a fact
hive.promote_fact("bob", HiveFact {
    fact_id: String::new(),
    content: "Nginx default port is 80".into(),
    concept: "networking".into(),
    confidence: 0.9,
});

// Query the hive
for fact in hive.query_facts("port networking", 10) {
    println!("  [{:.0}%] {}", fact.confidence * 100.0, fact.content);
}
```

## 2. Federation (Multiple Hives in a Tree)

Split agents across domain-specific hives, then query across the whole tree.

```rust
use amplihack_hive::graph::{InMemoryHiveGraph, HiveFact};

// Create a tree: root -> [security, infrastructure, data]
let mut root = InMemoryHiveGraph::new("root");

let mut security = InMemoryHiveGraph::new("security");
let mut infra    = InMemoryHiveGraph::new("infrastructure");
let mut data     = InMemoryHiveGraph::new("data");

for child in [&mut security, &mut infra, &mut data] {
    root.add_child(child);
    child.set_parent(&root);
}

// Each hive has its own agents and facts
security.register_agent("sec-1", "security");
security.promote_fact("sec-1", HiveFact {
    fact_id: String::new(),
    content: "CVE-2024-1234 affects OpenSSL 3.x".into(),
    concept: "vulnerabilities".into(),
    confidence: 0.95,
});

infra.register_agent("infra-1", "infrastructure");
infra.promote_fact("infra-1", HiveFact {
    fact_id: String::new(),
    content: "Server prod-db-01 runs on 10.0.1.5 port 5432".into(),
    concept: "servers".into(),
    confidence: 0.9,
});

data.register_agent("data-1", "data");
data.promote_fact("data-1", HiveFact {
    fact_id: String::new(),
    content: "Users table has 2.5M rows with daily growth of 10K".into(),
    concept: "schema".into(),
    confidence: 0.85,
});

// Federated query from root finds facts across ALL hives
for fact in root.query_federated("server port infrastructure", 10) {
    println!("  [{:.0}%] {}", fact.confidence * 100.0, fact.content);
}

// Federated query from a child also traverses the tree
let results = security.query_federated("server database port", 10);
println!("Security hive found {} results across federation", results.len());
```

## 3. LearningAgent with Hive Store (Recommended)

Connect a real LLM-backed `LearningAgent` to a shared hive for distributed
memory. Facts learned by the agent are auto-promoted to the hive; queries
merge local + hive facts.

```rust
use std::path::PathBuf;
use amplihack_agent_core::learning_agent::LearningAgent;
use amplihack_hive::graph::InMemoryHiveGraph;

// Create shared hive
let mut hive = InMemoryHiveGraph::new("shared-hive");
hive.register_agent("agent_a", "general");
hive.register_agent("agent_b", "general");

// Create agents with hive_store
let mut agent_a = LearningAgent::builder()
    .agent_name("agent_a")
    .storage_path(PathBuf::from("/tmp/agent_a_db"))
    .use_hierarchical(true)
    .hive_store(hive.clone())  // Auto-promotes facts to hive
    .build()?;

let mut agent_b = LearningAgent::builder()
    .agent_name("agent_b")
    .storage_path(PathBuf::from("/tmp/agent_b_db"))
    .use_hierarchical(true)
    .hive_store(hive.clone())
    .build()?;

// Agent A learns biology
agent_a.learn_from_content(
    "Photosynthesis converts sunlight into chemical energy in chloroplasts.",
)?;

// Agent B learns chemistry
agent_b.learn_from_content(
    "Water (H2O) has a bent molecular geometry with a 104.5 degree bond angle.",
)?;

// Agent B can answer questions about biology (facts shared via hive)
let answer = agent_b.answer_question("How do plants convert sunlight to energy?")?;
println!("{answer}");
```

**How auto-promotion works**: when `CognitiveAdapter::store_fact()` is called
(inside `learn_from_content`), it stores the fact in the agent's local
[`lbug`](../reference/ladybug-reference.md) graph DB **and** promotes it to
the hive. When `search()` or `get_all_facts()` is called (inside
`answer_question`), it queries both the local `lbug` store and the hive,
deduplicates by content, and returns merged results.

## 4. Distributed Agents with `lbug` (Full Stack)

Each agent owns its own `lbug` database. A shared `HiveGraphStore` acts as
the federation layer. `FederatedGraphStore` composes local + hive for
unified queries.

```rust
use amplihack_hive::distributed::{AgentNode, create_event_bus};

// Create event bus (local for testing, "redis" or "azure" for production)
let bus = create_event_bus("local");

// Create agents with their own lbug databases
let mut agent_a = AgentNode::builder()
    .agent_id("bio-agent")
    .domain("biology")
    .db_path("/tmp/hive_eval/bio-agent")
    .hive_store(None)        // or pass a HiveGraphStore for federation
    .event_bus(bus.clone())
    .build()?;

// Learn and query
agent_a.learn("Mitosis is cell division", &["biology"])?;
let results = agent_a.query("cell division")?;
println!("Found: {results:?}");
```

## 5. Deploy to Azure

The deploy flow provisions everything idempotently through the
`amplihack hive` CLI:

```bash
# Set your API key
export ANTHROPIC_API_KEY="your-api-key-here"  # pragma: allowlist secret

# Deploy 20 agents + 1 adversary to Azure Container Apps
amplihack hive deploy --agents 20 --adversary 1

# Check status
amplihack hive status

# Run the eval against deployed agents
amplihack hive eval --turns 5000 --questions 50

# Tear down when done
amplihack hive teardown
```

### What Gets Provisioned

| Resource           | Details                                                              |
| ------------------ | -------------------------------------------------------------------- |
| Resource Group     | `hive-mind-rg` (eastus)                                              |
| Container Registry | `hivacrhivemind` — Basic SKU, admin enabled                          |
| Service Bus        | `hive-sb-dj2qo2w7vu5zi` — Standard SKU, `hive-events` topic, 21 subs |
| Storage Account    | Azure Files share for `lbug` DB persistence                          |
| Container Apps     | 21 apps (20 domain + 1 adversary), 2.0 CPU / 4.0 GiB each            |

### Environment Overrides

```bash
export HIVE_RESOURCE_GROUP="my-rg"      # Default: hive-mind-rg
export HIVE_LOCATION="westus2"           # Default: eastus
export HIVE_AGENT_COUNT=10               # Default: 20
export HIVE_IMAGE_TAG="v2"               # Default: latest
```

## 6. Running the Learning Agent Eval

Compare single vs flat-sharing vs federated using real LLM agents:

```bash
# LearningAgent eval — real LLM fact extraction + synthesis
amplihack hive eval \
  --mode learning-agent \
  --turns 100 --questions 20 --agents 5 --groups 2

# Run just single condition (faster iteration)
amplihack hive eval \
  --mode learning-agent \
  --turns 50 --questions 10 --conditions single

# Use a specific model
amplihack hive eval \
  --mode learning-agent \
  --model claude-sonnet-4-5-20250929 \
  --turns 100 --questions 20
```

For keyword-based eval modes (no LLM, faster but less realistic):

```bash
# 20-agent distributed eval with lbug DBs
amplihack hive eval --mode distributed --agents 20

# 20-agent distributed eval with event bus
amplihack hive eval --mode full-distributed --agents 20
```

## Architecture

```
         Root Hive (InMemoryHiveGraph)
        ┌────┼────────┐─────────┐
    People Tech  Data  Ops  Misc
     Hive  Hive  Hive  Hive  Hive
      │     │     │     │     │
   agents agents agents agents agents
   (own    (own   (own   (own   (own
   lbug)  lbug)  lbug)  lbug)  lbug)
```

- Each agent owns its own [`lbug`](../reference/ladybug-reference.md) DB
  (private knowledge)
- Hive nodes are `HiveGraph` instances (shared knowledge)
- Federation enables recursive cross-tree queries
- `EventBus` propagates facts between agents (Local/Redis/Azure Service Bus)
- `HiveController` reconciles desired state from YAML manifests

## Key Modules

| Module                                                | Purpose                                           |
| ----------------------------------------------------- | ------------------------------------------------- |
| `crates/amplihack-hive/src/graph/`                    | `HiveGraph` trait, `InMemoryHiveGraph`, federation |
| `crates/amplihack-hive/src/controller.rs`             | `HiveController` (desired-state YAML manifests)   |
| `crates/amplihack-hive/src/distributed/`              | `AgentNode`, `HiveCoordinator`                    |
| `crates/amplihack-hive/src/event_bus.rs`              | `EventBus` trait + Local/Azure SB/Redis backends  |
| `crates/amplihack-hive/tests/`                        | Integration tests                                 |
| `crates/amplihack-cli/src/commands/hive_haymaker.rs`  | CLI bridge for `amplihack hive` subcommands       |
