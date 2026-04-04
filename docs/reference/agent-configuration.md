# Agent Configuration Reference

Complete reference for configuring agents, memory backends, evaluation,
and hive deployments in amplihack-rs.

## Agent Configuration

### Environment Variables

| Variable                    | Type     | Default             | Description                            |
|-----------------------------|----------|---------------------|----------------------------------------|
| `AMPLIHACK_AGENT_MODEL`     | `String` | `claude-sonnet-4-5` | Default LLM model for agents           |
| `AMPLIHACK_AGENT_BINARY`    | `String` | auto-detected       | Path to the AI tool binary             |
| `AMPLIHACK_MEMORY_BACKEND`  | `String` | `cognitive`         | Memory backend: `cognitive`, `hierarchical`, `memory` |
| `AMPLIHACK_MEMORY_TOPOLOGY` | `String` | `single`            | Memory topology: `single`, `distributed` |
| `AMPLIHACK_MEMORY_STORAGE_PATH` | `String` | `~/.amplihack/memory.db` | Memory storage path          |
| `AMPLIHACK_MAX_DEPTH`       | `u32`    | `3`                 | Max recursion depth for nested sessions |
| `AMPLIHACK_NONINTERACTIVE`  | `bool`   | `false`             | Skip interactive prompts               |
| `EVAL_MODEL`                | `String` | `claude-opus-4-6`   | Model used for evaluation grading      |

### Project Configuration File

Create `.amplihack/config.toml` in your project root:

```toml
[agent]
model = "claude-sonnet-4-5"
max_turns = 100
timeout_secs = 300

[memory]
backend = "cognitive"
topology = "single"
storage_path = ".amplihack/memory.db"

[memory.kuzu]  # backward-compatible config section name
buffer_pool_mb = 64
max_db_size_mb = 1024

[memory.distributed]
replication_factor = 3
query_fanout = 2
gossip_enabled = true
gossip_rounds = 3

[eval]
model = "claude-opus-4-6"
output_dir = "eval/results"

[hive]
num_containers = 20
agents_per_container = 5
location = "eastus"
topic_name = "hive-graph"
```

### User Configuration File

Create `~/.amplihack/config.toml` for user-level defaults:

```toml
[agent]
model = "claude-sonnet-4-5"

[memory]
backend = "cognitive"
```

## Memory Backend Configuration

### Cognitive Backend (Default)

```toml
[memory]
backend = "cognitive"
storage_path = "~/.amplihack/memory.db"
```

The cognitive backend stores memories using graph-based cognitive modeling with:
- Thread-safe access via `RwLock`
- Automatic schema migrations
- `0o600` file permissions
- Full-text search support

### LadybugDB Graph Backend

```toml
[memory]
backend = "kuzu"  # backward-compatible alias for the LadybugDB graph backend

[memory.kuzu]  # backward-compatible config section name
buffer_pool_mb = 64
max_db_size_mb = 1024
db_path = "~/.amplihack/kuzu_db"  # legacy path, backward-compatible
```

The LadybugDB (formerly Kuzu) backend provides graph storage for:
- Relationship-aware memory queries
- Code graph integration
- Cypher query support

**Requires**: Build with `lbug` feature flag.

### In-Memory Backend

```toml
[memory]
backend = "memory"
```

Volatile storage for testing. All data lost on restart.

### Distributed Topology

```toml
[memory]
topology = "distributed"

[memory.distributed]
replication_factor = 3
query_fanout = 2
gossip_enabled = true
gossip_rounds = 3
transport = "azure_service_bus"
connection_string = "Endpoint=sb://..."
```

Distributed topology enables:
- Content-based sharding via consistent hash ring
- Bloom filter gossip for cross-shard discovery
- Configurable replication and query fanout

## Evaluation Configuration

### Harness Settings

```toml
[eval]
model = "claude-opus-4-6"
output_dir = "eval/results"

[eval.teaching]
max_turns = 20
student_level = "intermediate"

[eval.long_horizon]
total_turns = 1000
checkpoint_interval = 100
categories = ["factual", "procedural", "episodic"]

[eval.self_improve]
iterations = 5
strategy = "prompt-refinement"
```

### Environment Variables for Eval

| Variable          | Type     | Default             | Description                    |
|-------------------|----------|---------------------|--------------------------------|
| `EVAL_MODEL`      | `String` | `claude-opus-4-6`   | Model for grading              |
| `EVAL_OUTPUT_DIR`  | `String` | `eval/results`     | Output directory               |
| `EVAL_MAX_TURNS`  | `u32`    | `1000`              | Max turns for long-horizon     |
| `EVAL_TIMEOUT`    | `u64`    | `3600`              | Per-evaluation timeout (secs)  |

## Hive Configuration

### Deployment Settings

```toml
[hive]
num_containers = 20
agents_per_container = 5
location = "eastus"
topic_name = "hive-graph"
cpu = 1.0
memory_gb = 4

[hive.azure]
resource_group = "hive-rg"
subscription_id = "your-sub-id"
acr_name = "myacr"
```

### Environment Variables for Hive

| Variable                                  | Type     | Default        | Description                    |
|-------------------------------------------|----------|----------------|--------------------------------|
| `SERVICE_BUS_CONN_STR`                    | `String` | (required)     | Service Bus connection string  |
| `AZURE_SUBSCRIPTION_ID`                   | `String` | (required)     | Azure subscription             |
| `AMPLIHACK_HIVE_NUM_CONTAINERS`           | `u32`    | `20`           | Container count override       |
| `AMPLIHACK_HIVE_AGENTS_PER_CONTAINER`     | `u32`    | `5`            | Agents per container override  |
| `AMPLIHACK_MEMORY_TRANSPORT`              | `String` | `local`        | Transport: `local`, `azure_service_bus` |

## Configuration Precedence

Configuration is resolved in this order (highest to lowest priority):

1. **Explicit API parameters** — passed directly to constructors
2. **Environment variables** — `AMPLIHACK_*` prefixed
3. **Project config** — `.amplihack/config.toml` in the working directory
4. **User config** — `~/.amplihack/config.toml`
5. **Compiled defaults** — hardcoded in the crate

## Related

- [Environment Variables](./environment-variables.md) — Full env var reference
- [Memory Backend](./memory-backend.md) — Backend selection details
- [Hive Orchestration](../concepts/hive-orchestration.md) — Hive architecture
