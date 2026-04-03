# amplihack-agent-generator

API reference for the `amplihack-agent-generator` crate — creating specialized
agents from natural-language goal descriptions.

## Crate Overview

`amplihack-agent-generator` implements the four-stage agent creation pipeline:
analyze → plan → synthesize → assemble. It produces standalone agent packages
from goal descriptions.

**Workspace dependency**: `amplihack-agent-generator = { path = "crates/amplihack-agent-generator" }`

## Modules

| Module              | Description                                        |
|---------------------|----------------------------------------------------|
| `prompt_analyzer`   | `PromptAnalyzer` — parse goals from natural language |
| `objective_planner` | `ObjectivePlanner` — phased execution planning     |
| `skill_synthesizer` | `SkillSynthesizer` — generate agent capabilities   |
| `agent_assembler`   | `AgentAssembler` — combine into bundles            |
| `packager`          | `GoalAgentPackager` — write to disk                |
| `models`            | All shared data types                              |
| `error`             | `GeneratorError` enum                              |

## Pipeline Stages

### Stage 1: PromptAnalyzer

```rust
impl PromptAnalyzer {
    pub fn new(model: &str) -> Self;
    pub fn analyze(&self, goal_text: &str) -> Result<GoalDefinition, GeneratorError>;
}
```

### Stage 2: ObjectivePlanner

```rust
impl ObjectivePlanner {
    pub fn new(model: &str) -> Self;
    pub fn plan(&self, goal: &GoalDefinition) -> Result<ExecutionPlan, GeneratorError>;
}
```

### Stage 3: SkillSynthesizer

```rust
impl SkillSynthesizer {
    pub fn new(model: &str) -> Self;
    pub fn synthesize(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<Vec<SkillDefinition>, GeneratorError>;
}
```

### Stage 4: AgentAssembler

```rust
impl AgentAssembler {
    pub fn new() -> Self;
    pub fn assemble(
        &self,
        goal: &GoalDefinition,
        plan: &ExecutionPlan,
        skills: &[SkillDefinition],
    ) -> Result<GoalAgentBundle, GeneratorError>;
    pub fn assemble_with_memory(
        &self,
        goal: &GoalDefinition,
        plan: &ExecutionPlan,
        skills: &[SkillDefinition],
        memory_config: MemoryConfig,
    ) -> Result<GoalAgentBundle, GeneratorError>;
}
```

### Packaging

```rust
impl GoalAgentPackager {
    pub fn new(output_dir: Option<PathBuf>) -> Self;
    pub fn package(&self, bundle: &GoalAgentBundle) -> Result<PathBuf, GeneratorError>;
}
```

## Data Types

### GoalDefinition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalDefinition {
    pub name: String,
    pub description: String,
    pub objectives: Vec<String>,
    pub constraints: Vec<String>,
    pub success_criteria: Vec<String>,
    pub required_capabilities: Vec<String>,
}
```

### ExecutionPlan

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub phases: Vec<PlanPhase>,
    pub estimated_complexity: Complexity,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPhase {
    pub order: u32,
    pub name: String,
    pub tasks: Vec<String>,
    pub dependencies: Vec<u32>,
    pub estimated_duration: Duration,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Complexity {
    Low,
    Medium,
    High,
    Critical,
}
```

### SkillDefinition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub implementation: String,
    pub required_tools: Vec<SDKToolConfig>,
    pub test_cases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SDKToolConfig {
    pub name: String,
    pub sdk: String,
    pub config: HashMap<String, Value>,
}
```

### GoalAgentBundle

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalAgentBundle {
    pub name: String,
    pub goal: GoalDefinition,
    pub plan: ExecutionPlan,
    pub skills: Vec<SkillDefinition>,
    pub sub_agents: Vec<SubAgentConfig>,
    pub memory_config: Option<MemoryConfig>,
}

impl GoalAgentBundle {
    pub fn is_complete(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    pub name: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub coordination_strategy: String,
}
```

## GeneratorError

```rust
#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    #[error("analysis failed: {0}")]
    Analysis(String),
    #[error("planning failed: {0}")]
    Planning(String),
    #[error("synthesis failed: {0}")]
    Synthesis(String),
    #[error("assembly failed: {0}")]
    Assembly(String),
    #[error("packaging failed: {0}")]
    Packaging(String),
    #[error("incomplete bundle: {0}")]
    IncompleteBundle(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
```

## Dependencies

| Crate                  | Purpose                    |
|------------------------|----------------------------|
| `amplihack-agent-core` | Agent types                |
| `amplihack-memory`     | Memory config types        |
| `serde`                | Serialization              |
| `serde_json`           | JSON handling              |
| `thiserror`            | Error derives              |
| `tracing`              | Structured logging         |
