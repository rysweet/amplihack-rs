# Goal Agent Generator

The goal agent generator (`amplihack-agent-generator`) creates specialized
autonomous agents from natural-language goal descriptions. It implements a
four-stage pipeline: analyze → plan → synthesize → assemble.

## Pipeline Architecture

```
Goal Description (natural language)
        │
        ▼
┌──────────────┐
│ PromptAnalyzer│  → GoalDefinition
└──────┬───────┘
       │
       ▼
┌──────────────────┐
│ ObjectivePlanner  │  → ExecutionPlan (phases, dependencies)
└──────┬───────────┘
       │
       ▼
┌──────────────────┐
│ SkillSynthesizer  │  → Vec<SkillDefinition> (code + config)
└──────┬───────────┘
       │
       ▼
┌──────────────────┐
│ AgentAssembler    │  → GoalAgentBundle (ready to package)
└──────┬───────────┘
       │
       ▼
┌──────────────────┐
│ GoalAgentPackager │  → Directory on disk (standalone agent)
└──────────────────┘
```

## Quick Start

```rust
use amplihack_agent_generator::{
    PromptAnalyzer, ObjectivePlanner, SkillSynthesizer,
    AgentAssembler, GoalAgentPackager,
};

// 1. Analyze the goal
let analyzer = PromptAnalyzer::new("claude-sonnet-4-5");
let goal = analyzer.analyze("Build an agent that monitors GitHub PRs and suggests reviewers based on code expertise")?;

// 2. Plan execution
let planner = ObjectivePlanner::new("claude-sonnet-4-5");
let plan = planner.plan(&goal)?;

// 3. Synthesize required skills
let synthesizer = SkillSynthesizer::new("claude-sonnet-4-5");
let skills = synthesizer.synthesize(&plan)?;

// 4. Assemble the agent
let assembler = AgentAssembler::new();
let bundle = assembler.assemble(&goal, &plan, &skills)?;

// 5. Package as standalone directory
let packager = GoalAgentPackager::new(Some("./output".into()));
let agent_dir = packager.package(&bundle)?;
println!("Agent packaged at: {}", agent_dir.display());
```

## Pipeline Stages

### Stage 1: Prompt Analysis

The `PromptAnalyzer` parses a natural-language goal into a structured
`GoalDefinition`:

```rust
use amplihack_agent_generator::{PromptAnalyzer, GoalDefinition};

let analyzer = PromptAnalyzer::new("claude-sonnet-4-5");
let goal: GoalDefinition = analyzer.analyze(
    "Create a code review agent that checks for security vulnerabilities"
)?;

println!("Name: {}", goal.name);
println!("Objectives: {:?}", goal.objectives);
println!("Constraints: {:?}", goal.constraints);
println!("Success criteria: {:?}", goal.success_criteria);
```

### Stage 2: Objective Planning

The `ObjectivePlanner` creates a phased execution plan with dependency
tracking:

```rust
use amplihack_agent_generator::{ObjectivePlanner, ExecutionPlan, PlanPhase};

let planner = ObjectivePlanner::new("claude-sonnet-4-5");
let plan: ExecutionPlan = planner.plan(&goal)?;

for phase in &plan.phases {
    println!("Phase {}: {} (depends on: {:?})",
        phase.order, phase.name, phase.dependencies);
}
```

### Stage 3: Skill Synthesis

The `SkillSynthesizer` generates the capabilities the agent needs:

```rust
use amplihack_agent_generator::{SkillSynthesizer, SkillDefinition};

let synthesizer = SkillSynthesizer::new("claude-sonnet-4-5");
let skills: Vec<SkillDefinition> = synthesizer.synthesize(&plan)?;

for skill in &skills {
    println!("Skill: {} (tools: {:?})", skill.name, skill.required_tools);
}
```

### Stage 4: Assembly

The `AgentAssembler` combines all components into a `GoalAgentBundle`:

```rust
use amplihack_agent_generator::{AgentAssembler, GoalAgentBundle};

let assembler = AgentAssembler::new();
let bundle: GoalAgentBundle = assembler.assemble(&goal, &plan, &skills)?;

assert!(bundle.is_complete());
println!("Bundle: {} ({} skills)", bundle.name, bundle.skills.len());
```

## Data Types

### GoalDefinition

```rust
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
pub struct ExecutionPlan {
    pub phases: Vec<PlanPhase>,
    pub estimated_complexity: Complexity,
    pub risk_factors: Vec<String>,
}

pub struct PlanPhase {
    pub order: u32,
    pub name: String,
    pub tasks: Vec<String>,
    pub dependencies: Vec<u32>,
    pub estimated_duration: Duration,
}
```

### SkillDefinition

```rust
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub implementation: String,  // Generated code
    pub required_tools: Vec<SDKToolConfig>,
    pub test_cases: Vec<String>,
}

pub struct SDKToolConfig {
    pub name: String,
    pub sdk: String,  // "claude", "openai", "local"
    pub config: HashMap<String, Value>,
}
```

### GoalAgentBundle

```rust
pub struct GoalAgentBundle {
    pub name: String,
    pub goal: GoalDefinition,
    pub plan: ExecutionPlan,
    pub skills: Vec<SkillDefinition>,
    pub sub_agents: Vec<SubAgentConfig>,
    pub memory_config: Option<MemoryConfig>,
    pub is_complete: bool,
}

pub struct SubAgentConfig {
    pub name: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub coordination_strategy: String,
}
```

## Packaging

The `GoalAgentPackager` creates a standalone directory with:

```
goal_agents/my-agent/
├── README.md              # Auto-generated documentation
├── goal.json              # GoalDefinition
├── plan.json              # ExecutionPlan
├── config.yaml            # Agent configuration
├── skills/
│   ├── skill_1.rs         # Generated skill implementations
│   └── skill_2.rs
├── sub_agents/            # (if multi-agent)
│   ├── coordinator.yaml
│   ├── spawner.yaml
│   └── memory_agent.yaml
└── main.rs                # Entry point
```

### Multi-Agent Packaging

When the plan calls for multiple cooperating agents, the packager
generates coordinator/spawner configurations:

```rust
let packager = GoalAgentPackager::new(Some("./output".into()));
let agent_dir = packager.package(&bundle)?;
// Creates sub_agents/ directory with YAML configs
```

## Memory Integration

Generated agents can optionally include memory configuration:

```rust
let bundle = assembler.assemble_with_memory(
    &goal, &plan, &skills,
    MemoryConfig {
        backend: Backend::Sqlite,
        topology: Topology::Single,
        ..Default::default()
    },
)?;
```

## Related

- [Agent Lifecycle](./agent-lifecycle.md) — How generated agents run
- [Evaluation Framework](./eval-framework.md) — Testing generated agents
- [Domain Agents](./domain-agents.md) — Pre-built specialized agents
