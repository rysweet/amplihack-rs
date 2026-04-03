# Domain Agents

Domain agents are specialized goal-seeking agents that inherit common
lifecycle management from `amplihack-agent-core` and add domain-specific
behavior: teaching pedagogy, code review analysis, meeting synthesis, and
more.

## Architecture

All domain agents implement the `DomainAgent` trait, which extends the
base agent with domain-specific evaluation and skill injection:

```
                 ┌──────────────┐
                 │ DomainAgent  │  (trait)
                 │  trait       │
                 └──────┬───────┘
                        │
          ┌─────────────┼──────────────┐
          │             │              │
   ┌──────┴──────┐ ┌───┴────┐  ┌──────┴────────┐
   │ TeachingAgent│ │CodeReview│ │MeetingSynthesizer│
   └─────────────┘ └────────┘  └────────────────┘
```

## The DomainAgent Trait

```rust
use amplihack_domain_agents::{DomainAgent, EvalLevel, EvalScenario, TaskResult};

pub trait DomainAgent: Send + Sync {
    /// Human-readable name for this agent type.
    fn name(&self) -> &str;

    /// Execute the agent's primary task.
    fn execute(&self, input: &str) -> Result<TaskResult, AgentError>;

    /// Evaluate the agent at a specific difficulty level.
    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError>;

    /// Return the evaluation levels this agent supports.
    fn supported_levels(&self) -> &[EvalLevel];
}
```

## Built-in Domain Agents

### TeachingAgent

Implements Socratic-method pedagogy for teaching technical concepts.
Uses a structured lesson plan with progressive disclosure.

```rust
use amplihack_domain_agents::teaching::TeachingAgent;

let teacher = TeachingAgent::new(TeachingConfig {
    subject: "Rust ownership".into(),
    difficulty: Difficulty::Intermediate,
    model: "claude-sonnet-4-5".into(),
    memory: session.memory_handle(),
});

let result = teacher.execute("Explain the borrow checker")?;
```

**Evaluation levels**: L1 (basic recall) through L6 (Socratic dialogue)

### CodeReviewAgent

Performs automated code review with configurable severity thresholds.
Analyzes diffs for bugs, security issues, and anti-patterns.

```rust
use amplihack_domain_agents::code_review::CodeReviewAgent;

let reviewer = CodeReviewAgent::new(CodeReviewConfig {
    severity_threshold: Severity::Warning,
    categories: vec![Category::Security, Category::Logic, Category::Performance],
    model: "claude-sonnet-4-5".into(),
});

let findings = reviewer.review_diff(diff_text)?;
```

**Evaluation levels**: L1 (syntax) through L4 (architectural)

### MeetingSynthesizerAgent

Extracts action items, decisions, and key points from meeting
transcripts. Produces structured summaries.

```rust
use amplihack_domain_agents::meeting::MeetingSynthesizerAgent;

let synthesizer = MeetingSynthesizerAgent::new(Default::default());
let summary = synthesizer.synthesize(transcript)?;
println!("Action items: {:?}", summary.action_items);
```

**Evaluation levels**: L1 (extraction) through L3 (cross-reference)

## Skill Injection

The `SkillInjector` registry allows dynamic capability injection into
any domain agent at runtime:

```rust
use amplihack_domain_agents::SkillInjector;

let mut injector = SkillInjector::new();
injector.register("web-search", web_search_skill);
injector.register("code-execution", code_exec_skill);

// Inject skills into an agent
let enhanced = injector.inject(base_agent)?;
```

Skills are composable — multiple skills can be injected into the same
agent, and the agent's OODA loop automatically discovers and uses
available skills during the Decide phase.

## Evaluation Framework Integration

Domain agents integrate with the `amplihack-agent-eval` evaluation
framework through the `EvalScenario` and `EvalLevel` types:

```rust
use amplihack_domain_agents::{EvalLevel, EvalScenario};
use amplihack_agent_eval::DomainEvalHarness;

let harness = DomainEvalHarness::new(vec![
    Box::new(teaching_agent),
    Box::new(code_review_agent),
]);

let report = harness.run_all()?;
for result in &report.results {
    println!("{}: {:.1}%", result.agent_name, result.score * 100.0);
}
```

## Creating Custom Domain Agents

Implement `DomainAgent` for your type and register it:

```rust
struct MyCustomAgent { /* ... */ }

impl DomainAgent for MyCustomAgent {
    fn name(&self) -> &str { "custom-analyzer" }

    fn execute(&self, input: &str) -> Result<TaskResult, AgentError> {
        // Your domain-specific logic
        Ok(TaskResult {
            output: format!("Analyzed: {}", input),
            confidence: 0.95,
            metadata: Default::default(),
        })
    }

    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError> {
        self.execute(&scenario.input)
    }

    fn supported_levels(&self) -> &[EvalLevel] {
        &[EvalLevel::L1, EvalLevel::L2]
    }
}
```

## Related

- [Agent Lifecycle](./agent-lifecycle.md) — Base agent state machine
- [Evaluation Framework](./eval-framework.md) — Progressive evaluation system
- [Hive Orchestration](./hive-orchestration.md) — Multi-agent coordination
