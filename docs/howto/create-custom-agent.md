# Create a Custom Agent

This guide walks you through creating a custom domain agent using the
amplihack-rs agent framework.

## Prerequisites

- amplihack-rs installed (`cargo install amplihack`)
- Rust 2024 edition or later

## Step 1: Add Dependencies

In your `Cargo.toml`:

```toml
[dependencies]
amplihack-agent-core = { version = "0.6" }
amplihack-domain-agents = { version = "0.6" }
amplihack-memory = { version = "0.6", features = ["sqlite"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Step 2: Define Your Agent

Create a struct that implements the `DomainAgent` trait:

```rust
use amplihack_agent_core::{AgentConfig, AgentError, TaskResult};
use amplihack_domain_agents::{DomainAgent, EvalLevel, EvalScenario};

pub struct SecurityAuditor {
    model: String,
}

impl SecurityAuditor {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
        }
    }
}

impl DomainAgent for SecurityAuditor {
    fn name(&self) -> &str {
        "security-auditor"
    }

    fn execute(&self, input: &str) -> Result<TaskResult, AgentError> {
        // Analyze the input for security issues
        let findings = analyze_security(input)?;

        Ok(TaskResult {
            output: format!("Found {} security issues", findings.len()),
            confidence: 0.9,
            metadata: serde_json::json!({
                "findings": findings,
            })
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        })
    }

    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError> {
        self.execute(&scenario.input)
    }

    fn supported_levels(&self) -> &[EvalLevel] {
        &[EvalLevel::L1, EvalLevel::L2, EvalLevel::L3]
    }
}
```

## Step 3: Add Memory Integration

Give your agent persistent memory:

```rust
use amplihack_memory::{Memory, MemoryOptions, Backend, Topology};

impl SecurityAuditor {
    pub fn with_memory(model: &str, agent_name: &str) -> Result<Self, AgentError> {
        let memory = Memory::new(agent_name, MemoryOptions {
            backend: Backend::Sqlite,
            topology: Topology::Single,
            ..Default::default()
        })?;

        Ok(Self {
            model: model.to_string(),
            memory: Some(memory),
        })
    }

    pub fn audit_with_context(&self, code: &str) -> Result<TaskResult, AgentError> {
        // Recall previous findings for context
        if let Some(ref mem) = self.memory {
            let history = mem.recall("previous security findings")?;
            // Use history to inform current analysis...
        }

        let result = self.execute(code)?;

        // Store findings for future reference
        if let Some(ref mem) = self.memory {
            mem.remember(&result.output)?;
        }

        Ok(result)
    }
}
```

## Step 4: Register with SkillInjector (Optional)

Add dynamic capabilities to your agent:

```rust
use amplihack_domain_agents::SkillInjector;

let mut injector = SkillInjector::new();

// Register a web search skill
injector.register("cve-lookup", Box::new(CveLookupSkill));

// Inject into your agent
let enhanced = injector.inject(Box::new(auditor))?;
```

## Step 5: Evaluate Your Agent

Run your agent through the evaluation framework:

```rust
use amplihack_agent_eval::DomainEvalHarness;

let harness = DomainEvalHarness::new(vec![Box::new(auditor)]);
let report = harness.run_all()?;

for result in &report.results {
    println!("{}: {:.1}% (levels: {:?})",
        result.agent_name,
        result.score * 100.0,
        result.level_scores.keys().collect::<Vec<_>>()
    );
}
```

## Step 6: Package as Standalone Agent

Use the goal agent generator to package:

```rust
use amplihack_agent_generator::GoalAgentPackager;

let packager = GoalAgentPackager::new(Some("./agents".into()));
let agent_dir = packager.package(&bundle)?;
println!("Packaged to: {}", agent_dir.display());
```

## Complete Example

See `examples/custom_security_agent.rs` for a full working example that
combines all these steps.

## Related

- [Agent Lifecycle](../concepts/agent-lifecycle.md) — State machine details
- [Domain Agents](../concepts/domain-agents.md) — Built-in agent types
- [Evaluation Framework](../concepts/eval-framework.md) — Testing agents
