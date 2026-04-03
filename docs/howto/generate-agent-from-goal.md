# Generate an Agent from a Goal

This guide shows how to use the goal agent generator to create a
specialized agent from a natural-language description.

## Prerequisites

- amplihack-rs installed
- An LLM API key configured (via `AMPLIHACK_AGENT_BINARY` or model config)

## Quick Start (CLI)

```bash
# Generate an agent from a goal description
amplihack generate-agent \
    --goal "Build an agent that monitors GitHub pull requests and suggests optimal reviewers based on code file expertise" \
    --output ./my-agents/ \
    --model claude-sonnet-4-5

# List generated files
ls my-agents/pr-reviewer-agent/
```

## Programmatic Usage

### Full Pipeline

```rust
use amplihack_agent_generator::{
    PromptAnalyzer, ObjectivePlanner, SkillSynthesizer,
    AgentAssembler, GoalAgentPackager,
};

let goal_text = "Create an agent that analyzes security logs, \
    identifies anomalous patterns, and generates incident reports";

// Stage 1: Analyze the goal
let analyzer = PromptAnalyzer::new("claude-sonnet-4-5");
let goal = analyzer.analyze(goal_text)?;
println!("Goal: {} ({} objectives)", goal.name, goal.objectives.len());

// Stage 2: Plan execution
let planner = ObjectivePlanner::new("claude-sonnet-4-5");
let plan = planner.plan(&goal)?;
println!("Plan: {} phases", plan.phases.len());

// Stage 3: Synthesize skills
let synthesizer = SkillSynthesizer::new("claude-sonnet-4-5");
let skills = synthesizer.synthesize(&plan)?;
println!("Skills: {}", skills.len());

// Stage 4: Assemble
let assembler = AgentAssembler::new();
let bundle = assembler.assemble(&goal, &plan, &skills)?;
assert!(bundle.is_complete());

// Package to disk
let packager = GoalAgentPackager::new(Some("./agents".into()));
let agent_dir = packager.package(&bundle)?;
println!("Agent written to: {}", agent_dir.display());
```

### With Memory Configuration

```rust
use amplihack_memory::{MemoryConfig, Backend, Topology};

let bundle = assembler.assemble_with_memory(
    &goal, &plan, &skills,
    MemoryConfig {
        backend: Backend::Sqlite,
        topology: Topology::Single,
        ..Default::default()
    },
)?;
```

### Multi-Agent Generation

When the goal requires coordination between multiple agents, the
generator automatically creates sub-agent configurations:

```rust
let goal_text = "Build a system where one agent indexes code repositories, \
    another agent answers questions about the code, and a coordinator \
    manages the workflow";

let goal = analyzer.analyze(goal_text)?;
let plan = planner.plan(&goal)?;
let skills = synthesizer.synthesize(&plan)?;
let bundle = assembler.assemble(&goal, &plan, &skills)?;

// Bundle includes sub-agent configurations
for sub in &bundle.sub_agents {
    println!("Sub-agent: {} (role: {})", sub.name, sub.role);
}

let agent_dir = packager.package(&bundle)?;
// Creates sub_agents/ directory with coordinator, spawner, memory configs
```

## Output Structure

The packager creates this directory structure:

```
my-agents/security-log-analyzer/
├── README.md              # Auto-generated documentation
├── goal.json              # Structured goal definition
├── plan.json              # Phased execution plan
├── config.yaml            # Runtime configuration
├── skills/
│   ├── log_parser.rs      # Log parsing skill
│   ├── anomaly_detector.rs # Anomaly detection
│   └── report_generator.rs # Report generation
└── main.rs                # Entry point
```

For multi-agent bundles:

```
my-agents/code-qa-system/
├── README.md
├── goal.json
├── plan.json
├── config.yaml
├── skills/
│   └── ...
├── sub_agents/
│   ├── coordinator.yaml   # Orchestration config
│   ├── indexer.yaml        # Code indexer agent
│   ├── responder.yaml      # QA responder agent
│   └── memory_agent.yaml   # Shared memory manager
└── main.rs
```

## Customizing Generated Agents

After generation, you can customize:

1. **Skills**: Edit files in `skills/` to refine behavior
2. **Configuration**: Modify `config.yaml` for model, memory, timeout settings
3. **Prompts**: Update system prompts in the config
4. **Dependencies**: Add tools or APIs to the skill definitions

## Evaluating Generated Agents

Test your generated agent:

```rust
use amplihack_agent_eval::DomainEvalHarness;

// Load the generated agent
let agent = load_generated_agent("./my-agents/security-log-analyzer")?;

let harness = DomainEvalHarness::new(vec![Box::new(agent)]);
let report = harness.run_all()?;
println!("Score: {:.1}%", report.results[0].score * 100.0);
```

## Related

- [Goal Agent Generator](../concepts/agent-generator.md) — Architecture
- [Create a Custom Agent](./create-custom-agent.md) — Manual agent creation
- [Run Agent Evaluations](./run-agent-evaluations.md) — Testing agents
