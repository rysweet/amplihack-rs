# Deploy a Hive Swarm

This guide walks through deploying a multi-agent hive swarm on Azure
Container Apps, feeding it content, querying it, and tearing it down.

## Prerequisites

- Azure subscription with Container Apps support
- Azure Container Registry (ACR) provisioned
- Azure Service Bus Premium namespace
- `az` CLI authenticated
- amplihack-rs installed

## Step 1: Configure the Hive

Create a hive configuration:

```rust
use amplihack_hive::{HiveMindWorkload, HiveConfig};

let config = HiveConfig {
    num_containers: 20,
    agents_per_container: 5,
    image: "myacr.azurecr.io/hive-agent:latest".into(),
    resource_group: "hive-rg".into(),
    subscription_id: "your-subscription-id".into(),
    location: "eastus".into(),
    acr_name: "myacr".into(),
    service_bus_connection_string: std::env::var("SERVICE_BUS_CONN_STR")?,
    topic_name: "hive-graph".into(),
    agent_prompt: "You are a learning agent in a distributed hive. \
        Memorize all content you receive and answer questions accurately.".into(),
    cpu: 1.0,
    memory_gb: 4,
};
```

### Environment Variables

| Variable                      | Description                        |
|-------------------------------|------------------------------------|
| `SERVICE_BUS_CONN_STR`        | Service Bus Premium connection     |
| `AZURE_SUBSCRIPTION_ID`       | Azure subscription                 |
| `AMPLIHACK_HIVE_NUM_CONTAINERS` | Override container count          |
| `AMPLIHACK_HIVE_AGENTS_PER_CONTAINER` | Override agents per container |

## Step 2: Deploy

```rust
let workload = HiveMindWorkload::new(config)?;
let deployment_id = workload.deploy()?;
println!("Deployment started: {}", deployment_id);
```

This will:
1. Build and push the Docker image to ACR
2. Deploy Service Bus topic and subscriptions via Bicep
3. Create `num_containers` Container Apps (named `hive-<id>-c01` through `hive-<id>-c20`)
4. Inject environment variables into each container

Monitor deployment progress:

```rust
loop {
    let status = workload.get_status(&deployment_id)?;
    println!("Ready: {}/{}", status.ready_containers, status.total_containers);
    if status.state == DeploymentState::Running {
        break;
    }
    std::thread::sleep(Duration::from_secs(10));
}
```

## Step 3: Feed Content

Feed learning content to the swarm:

```rust
use amplihack_agent_eval::collect_news;

// Load test data
let articles = collect_news(&websearch_data)?;

// Feed 100 turns to the swarm
workload.feed(&deployment_id, &articles, 100)?;
println!("Feed complete — agents are learning");
```

The feed publishes `LEARN_CONTENT` events to the Service Bus topic. Each
agent subscribes and memorizes the content. After all turns, a
`FEED_COMPLETE` sentinel is sent. Wait for `AGENT_READY` events before
querying.

## Step 4: Query the Swarm

```rust
let responses = workload.query(&deployment_id, "What is Rust's ownership model?")?;

for resp in &responses {
    println!("[Agent {}] (confidence: {:.2}) {}",
        resp.agent_id,
        resp.confidence,
        resp.answer
    );
}
```

## Step 5: Evaluate

Run the built-in benchmark:

```rust
use amplihack_hive::eval::HiveEval;

let eval = HiveEval::new(&workload, &deployment_id);
let report = eval.run_benchmark(&test_questions)?;

println!("Accuracy:       {:.1}%", report.accuracy * 100.0);
println!("Consensus rate: {:.1}%", report.consensus_rate * 100.0);
println!("Avg latency:    {:?}", report.avg_latency);
```

## Step 6: Teardown

```rust
// Stop containers (preserves state for later restart)
workload.stop(&deployment_id)?;

// Or delete everything
let report = workload.cleanup(&deployment_id)?;
println!("Deleted {} containers", report.containers_deleted);
```

## Cost Considerations

| Resource                | Estimated Cost (100 agents)       |
|-------------------------|-----------------------------------|
| 20 Container Apps (1 CPU, 4GB) | ~$0.50/hour                |
| Service Bus Premium     | ~$0.03/hour                       |
| ACR                     | Included in existing registry     |
| **Total**               | **~$0.53/hour**                   |

Stop or clean up deployments when not in use.

## Troubleshooting

### Containers not starting

```rust
let logs = workload.get_logs(&deployment_id)?;
println!("{}", logs);
```

### Service Bus connection failures

Ensure the connection string is for a **Premium** namespace. Standard
namespaces do not support the topic/subscription pattern used by the hive.

### Agents not receiving events

Check that `AMPLIHACK_MEMORY_TRANSPORT=azure_service_bus` is set in the
container environment.

## Related

- [Hive Orchestration](../concepts/hive-orchestration.md) — Architecture
- [Fleet Dashboard](./use-fleet-dashboard.md) — Monitoring agents
- [Memory Backend](../reference/memory-backend.md) — Agent memory config
