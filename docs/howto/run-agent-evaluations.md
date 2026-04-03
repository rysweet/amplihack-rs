# Run Agent Evaluations

This guide explains how to evaluate agent performance using the
amplihack-rs evaluation framework.

## Prerequisites

- amplihack-rs installed with the evaluation crate
- Test data prepared (news articles or custom test sets)

## Quick Evaluation

Run a complete evaluation harness:

```bash
# Prepare test data
amplihack eval prepare --news-file data/news.json --output-dir eval/

# Run full harness
amplihack eval run \
    --agent my-agent \
    --memory-backend sqlite \
    --news-file data/news.json \
    --output-dir eval/results/
```

## Programmatic Evaluation

### Basic Harness

```rust
use amplihack_agent_eval::{HarnessConfig, run_harness};

let config = HarnessConfig {
    news_file: "data/news.json".into(),
    output_dir: "eval/results/".into(),
    agent_name: "my-agent".into(),
    memory_backend: "sqlite".into(),
};

let result = run_harness(&config)?;
if result.success {
    println!("Overall score: {:.1}%", result.overall_score() * 100.0);
    if let Some(scores) = &result.scores {
        for (level, score) in scores {
            println!("  {}: {:.1}%", level, score * 100.0);
        }
    }
} else {
    eprintln!("Evaluation failed: {}", result.error_message.unwrap_or_default());
}
```

### Teaching Evaluation

```rust
use amplihack_agent_eval::{TeachingSession, TeachingConfig};

let session = TeachingSession::new(TeachingConfig {
    subject: "binary search algorithms".into(),
    student_level: "intermediate".into(),
    max_turns: 15,
    model: "claude-sonnet-4-5".into(),
});

let result = session.run()?;
println!("Clarity:     {:.2}", result.clarity);
println!("Engagement:  {:.2}", result.engagement);
println!("Correctness: {:.2}", result.correctness);
println!("Turns used:  {}", result.turns.len());
```

### Long-Horizon Memory Test

Stress-test memory retention across many turns:

```rust
use amplihack_agent_eval::{LongHorizonMemoryEval, LongHorizonConfig};

let eval = LongHorizonMemoryEval::new(LongHorizonConfig {
    total_turns: 1000,
    checkpoint_interval: 100,
    categories: vec![
        "factual".into(),
        "procedural".into(),
        "episodic".into(),
    ],
    model: "claude-sonnet-4-5".into(),
});

let report = eval.run(&my_agent)?;
println!("Overall retention: {:.1}%", report.overall_retention * 100.0);
for bd in &report.category_breakdowns {
    println!("  {}: {:.1}% ({}/{} retained)",
        bd.category,
        bd.retention_rate * 100.0,
        bd.retained_facts,
        bd.total_facts
    );
}
```

### Self-Improvement Loop

Run iterative improvement:

```rust
use amplihack_agent_eval::{run_long_horizon_self_improve, LongHorizonRunnerConfig};

let config = LongHorizonRunnerConfig {
    iterations: 5,
    eval_between_iterations: true,
    improvement_strategy: "prompt-refinement".into(),
    model: "claude-sonnet-4-5".into(),
    agent_name: "my-agent".into(),
    memory_backend: "sqlite".into(),
};

let result = run_long_horizon_self_improve(&config)?;
println!("Improvement: {:.1}% → {:.1}%",
    result.iteration_scores.first().unwrap_or(&0.0) * 100.0,
    result.final_score * 100.0
);
```

### General Capability Assessment

```rust
use amplihack_agent_eval::GeneralCapabilityEval;

let eval = GeneralCapabilityEval::new(Default::default());
let report = eval.run(&my_agent)?;

println!("Tool Use:      {:.2}", report.tool_use);
println!("Planning:      {:.2}", report.planning);
println!("Reasoning:     {:.2}", report.reasoning);
println!("Transfer:      {:.2}", report.transfer);
println!("Collaboration: {:.2}", report.collaboration);
println!("Overall:       {:.2}", report.overall);
```

## Reading Results

Evaluation results are written to the output directory as JSON:

```bash
eval/results/
├── harness_result.json    # Overall harness result
├── quiz.json              # Generated questions
├── answers.json           # Agent answers
├── grades.json            # Individual grades
└── summary.json           # Human-readable summary
```

### Result JSON Schema

```json
{
  "agent_name": "my-agent",
  "timestamp": "2026-04-03T00:00:00Z",
  "overall_score": 0.847,
  "level_scores": {
    "L1": 0.95,
    "L2": 0.90,
    "L3": 0.85
  },
  "questions_answered": 50,
  "questions_correct": 42,
  "duration_secs": 120
}
```

## Comparing Agents

Use `DomainEvalHarness` to compare multiple agents:

```rust
use amplihack_agent_eval::DomainEvalHarness;

let harness = DomainEvalHarness::new(vec![
    Box::new(agent_a),
    Box::new(agent_b),
    Box::new(agent_c),
]);

let report = harness.run_all()?;
for result in &report.results {
    println!("{:20} {:.1}%", result.agent_name, result.score * 100.0);
}
```

## Related

- [Evaluation Framework](../concepts/eval-framework.md) — Architecture and levels
- [Create a Custom Agent](./create-custom-agent.md) — Building agents to evaluate
- [Memory Backend](../reference/memory-backend.md) — Memory config for eval
