# Hive Orchestration

The hive system (`amplihack-hive`) provides multi-agent orchestration for
deploying and coordinating swarms of learning agents. It manages the full
lifecycle: deployment, event routing, content feeding, querying, and
teardown.

## Overview

A hive deployment creates N agent containers, each running M agents, for a
total of N×M cooperating agents that share knowledge through a distributed
event bus. The default topology is 20 containers × 5 agents = 100 agents.

```
┌──────────────────────────────────────────────┐
│                Hive Coordinator               │
│   (deployment, health, event routing)         │
├──────────────┬──────────────┬────────────────┤
│ Container 1  │ Container 2  │  Container N   │
│  Agent 1..M  │  Agent 1..M  │   Agent 1..M   │
├──────────────┴──────────────┴────────────────┤
│              Event Bus (topics)               │
│  LEARN_CONTENT │ FEED_COMPLETE │ QUERY/RESP   │
└──────────────────────────────────────────────┘
```

## Event System

Hive agents communicate through typed events on named topics:

| Event                 | Direction       | Purpose                                   |
|-----------------------|-----------------|-------------------------------------------|
| `hive.learn_content`  | Feed → Agents   | Content for agents to memorize            |
| `hive.feed_complete`  | Feed → Agents   | Sentinel: all content has been sent       |
| `hive.agent_ready`    | Agent → Coord.  | Agent finished processing all content     |
| `hive.query`          | User → Agents   | Question broadcast to the swarm           |
| `hive.query_response` | Agents → User   | Individual agent answers                  |

```rust
use amplihack_hive::events::{HiveEvent, EventTopic};

let event = HiveEvent::new(
    EventTopic::LEARN_CONTENT,
    serde_json::json!({
        "title": "Rust Memory Safety",
        "content": "Rust prevents data races at compile time..."
    }),
);
```

## Workload Configuration

The `HiveMindWorkload` implements the full deployment lifecycle:

```rust
use amplihack_hive::{HiveMindWorkload, HiveConfig};

let config = HiveConfig {
    num_containers: 20,
    agents_per_container: 5,
    image: "myregistry.azurecr.io/hive-agent:latest".into(),
    resource_group: "hive-rg".into(),
    subscription_id: "your-subscription-id".into(),
    location: "eastus".into(),
    acr_name: "myacr".into(),
    service_bus_connection_string: std::env::var("SERVICE_BUS_CONN_STR")
        .expect("SERVICE_BUS_CONN_STR required"),
    cpu: 1.0,
    memory_gb: 4,
    topic_name: "hive-graph".into(),
    agent_prompt: "You are a learning agent...".into(),
};

let workload = HiveMindWorkload::new(config)?;
```

### Configuration Reference

| Field                        | Type   | Default        | Description                           |
|------------------------------|--------|----------------|---------------------------------------|
| `num_containers`             | `u32`  | `20`           | Number of Container Apps to deploy    |
| `agents_per_container`       | `u32`  | `5`            | Agents per container                  |
| `image`                      | `String`| (required)    | Container image reference             |
| `resource_group`             | `String`| (required)    | Azure resource group                  |
| `subscription_id`            | `String`| (required)    | Azure subscription ID                 |
| `location`                   | `String`| `"eastus"`    | Azure region                          |
| `acr_name`                   | `String`| (required)    | Azure Container Registry name         |
| `service_bus_connection_string`| `String`| (required)  | Service Bus Premium connection string |
| `topic_name`                 | `String`| `"hive-graph"`| Service Bus topic                     |
| `agent_prompt`               | `String`| (optional)    | System prompt injected into agents    |
| `cpu`                        | `f64`  | `1.0`          | CPU cores per container               |
| `memory_gb`                  | `u32`  | `4`            | Memory per container (GiB)            |

## Deployment Lifecycle

### Deploy

```rust
let deployment_id = workload.deploy()?;
println!("Deployed: {}", deployment_id);
```

Deployment builds/pushes the Docker image, provisions Service Bus
infrastructure via Bicep, then creates each container app.

### Status

```rust
let status = workload.get_status(&deployment_id)?;
println!("State: {:?}, Containers: {}/{}",
    status.state,
    status.ready_containers,
    status.total_containers
);
```

### Feed Content

```rust
workload.feed(&deployment_id, &articles, 100)?;
// Publishes `turns` LEARN_CONTENT events, then FEED_COMPLETE
```

### Query the Swarm

```rust
let responses = workload.query(&deployment_id, "What is Rust?")?;
for resp in &responses {
    println!("[Agent {}] {}", resp.agent_id, resp.answer);
}
```

### Stop and Cleanup

```rust
workload.stop(&deployment_id)?;   // Sets min-replicas to 0
workload.cleanup(&deployment_id)?; // Deletes all container apps
```

## Evaluation Integration

The hive includes a built-in evaluation module for measuring swarm
performance:

```rust
use amplihack_hive::eval::HiveEval;

let eval = HiveEval::new(&workload, &deployment_id);
let report = eval.run_benchmark(&test_questions)?;
println!("Swarm accuracy: {:.1}%", report.accuracy * 100.0);
println!("Consensus rate: {:.1}%", report.consensus_rate * 100.0);
```

## Feed Module

The internal feed module publishes content events for agent learning:

```rust
use amplihack_hive::feed::run_feed;

run_feed(FeedConfig {
    deployment_id: &deployment_id,
    turns: 100,
    topic_name: "hive-graph",
    connection_string: &sb_conn_str,
    source: "amplihack-hive-feed",
})?;
```

## Workloads Submodule

Workloads are the execution units of the hive. Each workload type
implements the deployment, monitoring, and teardown contract:

```rust
use amplihack_hive::workload::WorkloadBase;

pub trait WorkloadBase {
    fn deploy(&self) -> Result<String, HiveError>;
    fn get_status(&self, deployment_id: &str) -> Result<DeploymentStatus, HiveError>;
    fn get_logs(&self, deployment_id: &str) -> Result<String, HiveError>;
    fn stop(&self, deployment_id: &str) -> Result<(), HiveError>;
    fn cleanup(&self, deployment_id: &str) -> Result<CleanupReport, HiveError>;
}
```

## Related

- [Agent Lifecycle](./agent-lifecycle.md) — Individual agent state machine
- [Fleet Dashboard Architecture](./fleet-dashboard-architecture.md) — Local/remote fleet management
- [Memory Backend Architecture](./memory-backend-architecture.md) — Shared memory for hive agents
