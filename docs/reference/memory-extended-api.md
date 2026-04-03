# amplihack-memory Extended API

API reference for the extended `amplihack-memory` crate — memory backends,
facade, manager, database, and evaluation framework.

## Extended Modules

These modules extend the existing `amplihack-memory` crate (which already
provides bloom filters, coordinator, distributed store, graph store,
hash ring, in-memory store, models, and quality scoring).

| Module         | Description                                            |
|----------------|--------------------------------------------------------|
| `facade`       | `Memory` — high-level remember/recall API              |
| `manager`      | `MemoryManager` — session-aware store/retrieve         |
| `database`     | `MemoryDatabase` — SQLite storage implementation       |
| `kuzu_store`   | `KuzuGraphStore` — Kuzu-backed graph storage           |
| `backends`     | Backend trait and auto-detection                       |
| `evaluation`   | Quality, performance, and reliability evaluators       |

## Memory Facade

The `Memory` struct provides the simplest API for agent memory:

```rust
use amplihack_memory::Memory;

let mem = Memory::new("my-agent", MemoryConfig {
    topology: Topology::Single,
    backend: Backend::Cognitive,
    ..Default::default()
})?;

// Store a fact
mem.remember("The API uses JWT for authentication")?;

// Recall relevant facts
let facts = mem.recall("authentication mechanism")?;
for fact in &facts {
    println!("{}: {}", fact.memory_type, fact.content);
}

mem.close()?;
```

### Memory

```rust
pub struct Memory {
    /* private fields */
}

impl Memory {
    pub fn new(agent_name: &str, config: MemoryConfig) -> Result<Self, MemoryError>;
    pub fn remember(&self, content: &str) -> Result<String, MemoryError>;
    pub fn recall(&self, query: &str) -> Result<Vec<RecallResult>, MemoryError>;
    pub fn facts(&self) -> Result<Vec<MemoryEntry>, MemoryError>;
    pub fn close(self) -> Result<(), MemoryError>;
    pub fn run_gossip(&self) -> Result<GossipResult, MemoryError>;
}
```

### MemoryConfig (existing)

The `Memory` facade reuses the existing `MemoryConfig` struct from the
memory crate:

```rust
pub struct MemoryConfig {
    pub topology: Topology,
    pub backend: Backend,         // Cognitive (default), Hierarchical, InMemory
    pub transport: Transport,
    pub storage_path: Option<PathBuf>,
    pub replication_factor: u32,
    pub query_fanout: u32,
    pub gossip_enabled: bool,
    pub token_budget: usize,
}
```

### RecallResult

```rust
pub struct RecallResult {
    pub entry: MemoryEntry,
    pub relevance: f64,
    pub source: String,
}
```

## Memory Manager

Session-aware memory operations with automatic lifecycle management:

```rust
use amplihack_memory::MemoryManager;

let manager = MemoryManager::new(
    Some("/path/to/memory.db"),
    Some("session-123"),
)?;

let id = manager.store(
    "agent-1",
    "Important discovery",
    "The codebase uses the repository pattern",
    MemoryType::Context,
    None,     // metadata
    Some(vec!["architecture".into()]),  // tags
    Some(8),  // importance (1-10)
    None,     // expires_in
    None,     // parent_id
)?;

let entries = manager.retrieve(&MemoryQuery {
    agent_id: Some("agent-1".into()),
    memory_types: vec![MemoryType::Context],
    tags: Some(vec!["architecture".into()]),
    limit: Some(10),
    ..Default::default()
})?;
```

### MemoryManager

```rust
impl MemoryManager {
    pub fn new(
        db_path: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Self, MemoryError>;

    pub fn store(
        &self,
        agent_id: &str,
        title: &str,
        content: &str,
        memory_type: MemoryType,
        metadata: Option<HashMap<String, Value>>,
        tags: Option<Vec<String>>,
        importance: Option<f64>,
        expires_in: Option<Duration>,
        parent_id: Option<&str>,
    ) -> Result<String, MemoryError>;

    pub fn retrieve(
        &self,
        query: &MemoryQuery,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;

    pub fn delete(&self, memory_id: &str) -> Result<bool, MemoryError>;

    pub fn get_session_info(&self) -> Result<SessionInfo, MemoryError>;

    pub fn export(&self, format: ExportFormat) -> Result<String, MemoryError>;

    pub fn close(self) -> Result<(), MemoryError>;
}
```

## Memory Database

Thread-safe SQLite implementation:

```rust
use amplihack_memory::MemoryDatabase;

let db = MemoryDatabase::new(Some("/path/to/memory.db"))?;
db.initialize()?;
```

### MemoryDatabase

```rust
impl MemoryDatabase {
    pub fn new(db_path: Option<&str>) -> Result<Self, MemoryError>;
    pub fn initialize(&self) -> Result<(), MemoryError>;
    pub fn store_memory(&self, entry: &MemoryEntry) -> Result<(), MemoryError>;
    pub fn query_memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;
    pub fn delete_memory(&self, id: &str) -> Result<bool, MemoryError>;
    pub fn get_session_info(&self, session_id: &str) -> Result<SessionInfo, MemoryError>;
    pub fn vacuum(&self) -> Result<(), MemoryError>;
    pub fn close(self) -> Result<(), MemoryError>;
}
```

**Security**: Database files are created with `0o600` permissions (owner
read/write only). Error messages are sanitized to prevent information leakage.

## Kuzu Graph Store

Embedded graph database backend using Kùzu:

```rust
use amplihack_memory::KuzuGraphStore;

let store = KuzuGraphStore::new(
    Some("/path/to/kuzu_db"),
    64 * 1024 * 1024,   // buffer pool: 64 MB
    1024 * 1024 * 1024,  // max db size: 1 GB
)?;

store.create_node("Memory", &[
    ("node_id", "mem-1"),
    ("content", "The API uses JWT"),
    ("memory_type", "context"),
])?;

let results = store.search_nodes("Memory", "content", "JWT")?;
```

### KuzuGraphStore

```rust
impl KuzuGraphStore {
    pub fn new(
        db_path: Option<&str>,
        buffer_pool_size: usize,
        max_db_size: usize,
    ) -> Result<Self, MemoryError>;

    pub fn create_node(
        &self,
        table: &str,
        properties: &[(&str, &str)],
    ) -> Result<String, MemoryError>;

    pub fn get_node(
        &self,
        table: &str,
        node_id: &str,
    ) -> Result<Option<HashMap<String, Value>>, MemoryError>;

    pub fn query_nodes(
        &self,
        table: &str,
        filter: &str,
        limit: usize,
    ) -> Result<Vec<HashMap<String, Value>>, MemoryError>;

    pub fn search_nodes(
        &self,
        table: &str,
        field: &str,
        text: &str,
    ) -> Result<Vec<HashMap<String, Value>>, MemoryError>;

    pub fn create_edge(
        &self,
        from_table: &str,
        from_id: &str,
        to_table: &str,
        to_id: &str,
        rel_type: &str,
    ) -> Result<(), MemoryError>;

    pub fn get_edges(
        &self,
        table: &str,
        node_id: &str,
        rel_type: &str,
    ) -> Result<Vec<HashMap<String, Value>>, MemoryError>;

    pub fn close(self) -> Result<(), MemoryError>;
}
```

**Requires**: `kuzu` feature flag enabled.

## Evaluation Framework

### QualityEvaluator

```rust
impl QualityEvaluator {
    pub fn new() -> Self;
    pub fn evaluate(
        &self,
        store: &dyn GraphStore,
        test_data: &[TestCase],
    ) -> Result<QualityMetrics, MemoryError>;
}

pub struct QualityMetrics {
    pub relevance: f64,
    pub precision: f64,
    pub recall: f64,
    pub ranking_quality: f64,
}
```

### PerformanceEvaluator

```rust
impl PerformanceEvaluator {
    pub fn new() -> Self;
    pub fn evaluate(
        &self,
        store: &dyn GraphStore,
        operations: u32,
    ) -> Result<PerformanceMetrics, MemoryError>;
}

pub struct PerformanceMetrics {
    pub avg_write_latency: Duration,
    pub avg_read_latency: Duration,
    pub throughput_ops_per_sec: f64,
    pub p99_latency: Duration,
}
```

### ReliabilityEvaluator

```rust
impl ReliabilityEvaluator {
    pub fn new() -> Self;
    pub fn evaluate(
        &self,
        store: &dyn GraphStore,
    ) -> Result<ReliabilityMetrics, MemoryError>;
}

pub struct ReliabilityMetrics {
    pub data_integrity: f64,
    pub concurrent_safety: f64,
    pub recovery_success: f64,
}
```

### BackendComparison

```rust
impl BackendComparison {
    pub fn new(backends: Vec<Box<dyn GraphStore>>) -> Self;
    pub fn run(&self) -> Result<ComparisonReport, MemoryError>;
}

pub struct ComparisonReport {
    pub quality: HashMap<String, QualityMetrics>,
    pub performance: HashMap<String, PerformanceMetrics>,
    pub reliability: HashMap<String, ReliabilityMetrics>,
    pub recommendation: String,
}

pub fn run_evaluation(
    backends: Vec<Box<dyn GraphStore>>,
) -> Result<ComparisonReport, MemoryError>;
```

## Feature Flags

| Flag     | Enables                                          |
|----------|--------------------------------------------------|
| `sqlite` | SQLite-backed `MemoryDatabase` (via `rusqlite`)  |
| `kuzu`   | Kuzu-backed `KuzuGraphStore` (via `kuzu` crate)  |
| `redis`  | Redis transport for distributed topology         |

Default features: `["sqlite"]`

## Dependencies

| Crate      | Purpose                    | Feature    |
|------------|----------------------------|------------|
| `serde`    | Serialization              | always     |
| `serde_json`| JSON handling             | always     |
| `thiserror`| Error derives              | always     |
| `tracing`  | Structured logging         | always     |
| `rusqlite` | SQLite storage             | `sqlite`   |
| `kuzu`     | Graph database             | `kuzu`     |
| `uuid`     | Memory ID generation       | always     |
| `chrono`   | Timestamps                 | always     |
