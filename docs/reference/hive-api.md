# amplihack-hive

API reference for the `amplihack-hive` crate — multi-agent orchestration,
workload management, and distributed agent swarms.

## Crate Overview

`amplihack-hive` manages the deployment and coordination of agent swarms on
Azure Container Apps. It handles the full workload lifecycle: deployment,
content feeding, querying, evaluation, and teardown.

**Workspace dependency**: `amplihack-hive = { path = "crates/amplihack-hive" }`

## Modules

| Module     | Description                                          |
|------------|------------------------------------------------------|
| `workload` | `HiveMindWorkload`, `HiveConfig`, deployment lifecycle |
| `events`   | `HiveEvent`, `EventTopic`, typed event constants     |
| `feed`     | `run_feed`, `FeedConfig` — content publishing        |
| `eval`     | `HiveEval`, `run_eval` — swarm evaluation            |
| `error`    | `HiveError` enum                                     |

## HiveMindWorkload

### HiveConfig

```rust
pub struct HiveConfig {
    pub num_containers: u32,
    pub agents_per_container: u32,
    pub image: String,
    pub resource_group: String,
    pub subscription_id: String,
    pub location: String,
    pub acr_name: String,
    pub service_bus_connection_string: String,
    pub topic_name: String,
    pub agent_prompt: String,
    pub cpu: f64,
    pub memory_gb: u32,
}

impl Default for HiveConfig {
    fn default() -> Self {
        Self {
            num_containers: 20,
            agents_per_container: 5,
            image: String::new(),
            resource_group: String::new(),
            subscription_id: String::new(),
            location: "eastus".into(),
            acr_name: String::new(),
            service_bus_connection_string: String::new(),
            topic_name: "hive-graph".into(),
            agent_prompt: String::new(),
            cpu: 1.0,
            memory_gb: 4,
        }
    }
}
```

### HiveMindWorkload

```rust
impl HiveMindWorkload {
    pub fn new(config: HiveConfig) -> Result<Self, HiveError>;

    /// Deploy N container apps, each running M agents.
    /// Returns a deployment ID for subsequent operations.
    pub fn deploy(&self) -> Result<String, HiveError>;

    /// Query deployment status.
    pub fn get_status(&self, deployment_id: &str) -> Result<DeploymentStatus, HiveError>;

    /// Stream logs from the first container app.
    pub fn get_logs(&self, deployment_id: &str) -> Result<String, HiveError>;

    /// Stop all container apps (set min-replicas to 0).
    pub fn stop(&self, deployment_id: &str) -> Result<(), HiveError>;

    /// Delete all container apps for this deployment.
    pub fn cleanup(&self, deployment_id: &str) -> Result<CleanupReport, HiveError>;

    /// Feed content to the swarm for learning.
    pub fn feed(
        &self,
        deployment_id: &str,
        articles: &[NewsArticle],
        turns: u32,
    ) -> Result<(), HiveError>;

    /// Query the swarm and collect responses.
    pub fn query(
        &self,
        deployment_id: &str,
        question: &str,
    ) -> Result<Vec<QueryResponse>, HiveError>;
}
```

### DeploymentStatus

```rust
pub struct DeploymentStatus {
    pub state: DeploymentState,
    pub ready_containers: u32,
    pub total_containers: u32,
    pub deployment_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentState {
    Provisioning,
    Running,
    Stopped,
    Failed,
    Cleaned,
}
```

### CleanupReport

```rust
pub struct CleanupReport {
    pub containers_deleted: u32,
    pub errors: Vec<String>,
    pub duration: Duration,
}
```

### QueryResponse

```rust
pub struct QueryResponse {
    pub agent_id: String,
    pub answer: String,
    pub confidence: f64,
    pub latency: Duration,
}
```

## Event System

### EventTopic

```rust
pub struct EventTopic;

impl EventTopic {
    pub const LEARN_CONTENT: &str = "hive.learn_content";
    pub const FEED_COMPLETE: &str = "hive.feed_complete";
    pub const AGENT_READY: &str = "hive.agent_ready";
    pub const QUERY: &str = "hive.query";
    pub const QUERY_RESPONSE: &str = "hive.query_response";
}
```

### HiveEvent

```rust
pub struct HiveEvent {
    pub topic: String,
    pub payload: Value,
    pub source: String,
    pub timestamp: DateTime<Utc>,
    pub deployment_id: String,
}

impl HiveEvent {
    /// Creates an event. `source` defaults to the caller's agent ID
    /// and `deployment_id` must be provided separately.
    pub fn new(topic: &str, payload: Value, deployment_id: &str) -> Self;
}
```

### Event Factory Functions

```rust
pub fn make_learn_content_event(content: &str, deployment_id: &str) -> HiveEvent;
pub fn make_feed_complete_event(deployment_id: &str) -> HiveEvent;
pub fn make_agent_ready_event(agent_id: &str, deployment_id: &str) -> HiveEvent;
pub fn make_query_event(question: &str, deployment_id: &str) -> HiveEvent;
pub fn make_query_response_event(
    agent_id: &str,
    answer: &str,
    deployment_id: &str,
) -> HiveEvent;
```

## Feed Module

### FeedConfig

```rust
pub struct FeedConfig<'a> {
    pub deployment_id: &'a str,
    pub turns: u32,
    pub topic_name: &'a str,
    pub connection_string: &'a str,
    pub source: &'a str,
}
```

### run_feed

```rust
pub fn run_feed(config: FeedConfig<'_>) -> Result<(), HiveError>;
```

Publishes `turns` `LEARN_CONTENT` events followed by a `FEED_COMPLETE` sentinel.

## Evaluation Module

### HiveEval

```rust
impl HiveEval {
    pub fn new(workload: &HiveMindWorkload, deployment_id: &str) -> Self;
    pub fn run_benchmark(
        &self,
        questions: &[QuizQuestion],
    ) -> Result<HiveEvalReport, HiveError>;
}

pub struct HiveEvalReport {
    pub accuracy: f64,
    pub consensus_rate: f64,
    pub avg_latency: Duration,
    pub per_question: Vec<QuestionResult>,
}

pub struct QuestionResult {
    pub question: String,
    pub responses: Vec<QueryResponse>,
    pub consensus_answer: Option<String>,
    pub correct: bool,
}
```

## HiveError

```rust
#[derive(Debug, thiserror::Error)]
pub enum HiveError {
    #[error("deployment error: {0}")]
    Deployment(String),
    #[error("event bus error: {0}")]
    EventBus(String),
    #[error("container error: {0}")]
    Container(String),
    #[error("query timeout after {0:?}")]
    QueryTimeout(Duration),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
```

## Dependencies

| Crate                  | Purpose                      |
|------------------------|------------------------------|
| `amplihack-agent-core` | Agent types                  |
| `amplihack-agent-eval` | Evaluation integration       |
| `amplihack-memory`     | Shared memory                |
| `serde`                | Serialization                |
| `serde_json`           | JSON handling                |
| `thiserror`            | Error derives                |
| `tracing`              | Structured logging           |
| `chrono`               | Timestamps                   |
