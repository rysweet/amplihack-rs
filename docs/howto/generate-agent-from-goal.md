# Generate an Agent from a Goal

This guide shows how to use the goal agent generator to create a
specialized agent from a natural-language description.

## Prerequisites

- amplihack-rs installed
- An LLM API key configured (via `AMPLIHACK_AGENT_BINARY` or model config)

## Quick Start

The generator is currently a library API. CLI integration is planned for a
future release. Use the programmatic API below.

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
        backend: Backend::Cognitive,
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
├── agent_config.json      # Runtime configuration
├── prompt.md              # System prompt
├── skills/
│   ├── log_parser.yaml    # Log parsing skill config
│   ├── anomaly_detector.yaml # Anomaly detection config
│   └── report_generator.yaml # Report generation config
├── requirements.txt       # Dependencies (Python SDK)
└── main.py                # Entry point (SDK-dependent)
```

For multi-agent bundles:

```
my-agents/code-qa-system/
├── README.md
├── goal.json
├── plan.json
├── agent_config.json
├── prompt.md
├── skills/
│   └── ...
├── sub_agents/
│   ├── coordinator.yaml   # Orchestration config
│   ├── indexer.yaml        # Code indexer agent
│   ├── responder.yaml      # QA responder agent
│   └── memory_agent.yaml   # Shared memory manager
├── requirements.txt
└── main.py
```

## Customizing Generated Agents

After generation, you can customize:

1. **Skills**: Edit files in `skills/` to refine behavior
2. **Configuration**: Modify `config.yaml` for model, memory, timeout settings
3. **Prompts**: Update system prompts in the config
4. **Dependencies**: Add tools or APIs to the skill definitions

## Evaluating Generated Agents

Test your generated agent by loading and evaluating it:

```rust
use amplihack_agent_eval::DomainEvalHarness;
use amplihack_agent_generator::GoalAgentPackager;

// The generated agent directory contains config that can be loaded
// into a DomainAgent implementation for evaluation.
let harness = DomainEvalHarness::new(vec![Box::new(my_agent)]);
let report = harness.run_all()?;
println!("Score: {:.1}%", report.results[0].score * 100.0);
```

## Related

- [Goal Agent Generator](../concepts/agent-generator.md) — Architecture
- [Create a Custom Agent](./create-custom-agent.md) — Manual agent creation
- [Run Agent Evaluations](./run-agent-evaluations.md) — Testing agents
