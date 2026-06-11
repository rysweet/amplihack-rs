# Fault-Tolerance Pattern Orchestrators

This directory contains three fault-tolerance pattern orchestrators that provide robust, reliable execution strategies for AI-powered development workflows.

## Overview

Each pattern addresses a different aspect of reliability:

1. **N-Version Programming** - Reduces errors through diversity
2. **Multi-Agent Debate** - Improves decisions through multiple perspectives
3. **Fallback Cascade** - Ensures completion through graceful degradation

## Patterns

### 1. N-Version Programming (`n_version.rs`)

Generate N independent implementations in parallel, compare them, and select the best.

**When to use:**

- Critical security features (authentication, authorization)
- Complex algorithms with multiple valid approaches
- High-risk refactoring of core components
- When correctness is paramount

**Example:**

```rust
use amplihack_orchestration::patterns::n_version::run_n_version;

let result = run_n_version(RunNVersionConfig {
    task_prompt: "Implement password hashing function".into(),
    n: 3,
    selection_criteria: vec!["security".into(), "correctness".into(), "simplicity".into()],
    timeout: Some(300),
    ..Default::default()
});

println!("Selected: {}", result.selected);
println!("Rationale: {}", result.rationale);
```

**Based on:** `~/.amplihack/.claude/workflow/N_VERSION_WORKFLOW.md`

**Key Features:**

- Parallel execution of N implementations
- Diversity through different implementation profiles (conservative, pragmatic, minimalist, etc.)
- Automated comparison and selection using reviewer agent
- Support for hybrid synthesis (combining best parts)

**Returns:**

```rust
NVersionResult {
    versions: Vec<ProcessResult>,      // All N implementation outputs
    comparison: ProcessResult,          // Reviewer analysis
    selected: String,                   // "version_1", "version_2", or "hybrid"
    rationale: String,                  // Selection explanation
    session_id: String,                 // For log tracking
    success: bool,                      // Overall success
}
```

---

### 2. Multi-Agent Debate (`debate.rs`)

Conduct structured debate with multiple perspectives to reach consensus on complex decisions.

**When to use:**

- Major architectural decisions
- Complex trade-offs with no clear winner
- Controversial changes affecting multiple teams
- Decisions that are expensive to reverse

**Example:**

```rust
use amplihack_orchestration::patterns::debate::run_debate;

let result = run_debate(RunDebateConfig {
    decision_question: "Should we use PostgreSQL or MongoDB?".into(),
    perspectives: vec!["security".into(), "performance".into(), "simplicity".into(), "cost".into()],
    rounds: 3,
    timeout: Some(180),
    ..Default::default()
});

println!("Consensus: {}", result.synthesis.output);
println!("Confidence: {}", result.confidence);
```

**Based on:** `~/.amplihack/.claude/workflow/DEBATE_WORKFLOW.md`

**Key Features:**

- Multiple perspectives (security, performance, simplicity, maintainability, etc.)
- Structured rounds: initial positions → challenges → synthesis
- Parallel execution within each round
- Facilitator synthesis with confidence levels
- Documents dissenting views

**Returns:**

```rust
DebateResult {
    rounds: Vec<RoundResult>,                 // Each round's results
    positions: HashMap<String, Vec<String>>,   // Position history per perspective
    synthesis: ProcessResult,                  // Final consensus
    confidence: String,                        // "HIGH", "MEDIUM", or "LOW"
    session_id: String,                        // For log tracking
    success: bool,                             // Overall success
}
```

---

### 3. Fallback Cascade (`cascade.rs`)

Graceful degradation through cascading fallback strategies.

**When to use:**

- External service dependencies
- Time-sensitive operations with acceptable degraded modes
- High-availability requirements
- When partial results are better than no results

**Example:**

```rust
use amplihack_orchestration::patterns::cascade::run_cascade;

let result = run_cascade(RunCascadeConfig {
    task_prompt: "Generate API documentation".into(),
    fallback_strategy: "quality".into(),      // or "service", "freshness", "completeness"
    timeout_strategy: "balanced".into(),      // or "aggressive", "patient"
    notification_level: "explicit".into(),
    ..Default::default()
});

println!("Succeeded at {} level", result.cascade_level);
if !result.degradation.is_empty() {
    println!("Degradation: {}", result.degradation);
}
```

**Based on:** `~/.amplihack/.claude/workflow/CASCADE_WORKFLOW.md`

**Key Features:**

- Three levels: primary (optimal) → secondary (acceptable) → tertiary (minimal)
- Predefined strategies: quality, service, freshness, completeness, accuracy
- Timeout strategies: aggressive, balanced, patient
- Notification levels: silent, warning, explicit
- Custom cascade support via `create_custom_cascade()`

**Returns:**

```rust
CascadeResult {
    result: ProcessResult,              // Final successful result
    cascade_level: String,              // "primary", "secondary", "tertiary", or "failed"
    degradation: String,                // Degradation description (if any)
    attempts: Vec<ProcessResult>,       // All attempts made
    session_id: String,                 // For log tracking
    success: bool,                      // Whether any level succeeded
}
```

---

## Common Parameters

All patterns share these common parameters:

- `working_dir: Option<PathBuf>` - Working directory (default: current dir)
- `model: Option<String>` - Claude model to use (default: CLI default)
- `timeout: Option<u64>` - Timeout per process in seconds

## Session Logs

All patterns create session logs in `~/.amplihack/.claude/runtime/logs/<session_id>/`:

- `session.log` - Overall session information
- `<process_id>.log` - Individual process logs

Session IDs are returned in results for traceability.

## Pattern Selection Guide

| Scenario                  | Pattern   | Why                                                |
| ------------------------- | --------- | -------------------------------------------------- |
| Critical security feature | N-Version | Multiple implementations catch vulnerabilities     |
| Architecture decision     | Debate    | Multiple perspectives surface trade-offs           |
| External API integration  | Cascade   | Graceful degradation when service slow/unavailable |
| Complex algorithm         | N-Version | Different approaches reveal design insights        |
| Controversial change      | Debate    | Structured discussion builds consensus             |
| Time-sensitive operation  | Cascade   | Acceptable degradation better than timeout         |

## Examples

See `examples/patterns.rs` for complete working examples of each pattern.

## Integration with Workflows

These patterns are designed to integrate with the `default-workflow` skill/recipe:

- **N-Version**: Replaces Steps 4-5 (Research/Design and Implementation)
- **Debate**: Replaces Step 4 (Research and Design)
- **Cascade**: Can be applied to any step with fallback options

## Architecture

All patterns are built on the orchestration infrastructure:

```
patterns/
├── n_version.rs        # N-Version Programming orchestrator
├── debate.rs           # Multi-Agent Debate orchestrator
├── cascade.rs          # Fallback Cascade orchestrator
└── mod.rs              # Pattern exports

Uses:
../claude_process.rs    # Subprocess wrapper
../execution.rs         # Parallel/sequential/fallback helpers
../session.rs           # Session management
```

## Philosophy Alignment

These patterns follow Amplihack's core principles:

- **Ruthless Simplicity** - Simple, direct implementations
- **Zero-BS** - No stubs, no placeholders, working code only
- **Fault Tolerance** - Graceful handling of failures
- **Transparency** - Clear logging and rationale
- **Evidence-Based** - Decisions backed by data and analysis

## Advanced Usage

### Custom Cascade Levels

```rust
use amplihack_orchestration::patterns::cascade::create_custom_cascade;

let result = create_custom_cascade(CreateCustomCascadeConfig {
    task_prompt: "Analyze code".into(),
    levels: vec![
        CascadeLevel {
            name: "deep_analysis".into(),
            timeout: 120,
            constraint: "comprehensive analysis with all recommendations".into(),
            ..Default::default()
        },
        CascadeLevel {
            name: "quick_scan".into(),
            timeout: 30,
            constraint: "identify major issues only".into(),
            ..Default::default()
        },
        CascadeLevel {
            name: "syntax_check".into(),
            timeout: 5,
            constraint: "basic syntax validation".into(),
            ..Default::default()
        },
    ],
    ..Default::default()
});
```

### Custom N-Version Profiles

```rust
let result = run_n_version(RunNVersionConfig {
    task_prompt: "Implement feature".into(),
    diversity_profiles: Some(vec![
        DiversityProfile {
            name: "security_first".into(),
            traits: "Prioritize security over all else, defensive programming".into(),
        },
        DiversityProfile {
            name: "performance_first".into(),
            traits: "Optimize for speed, minimize allocations".into(),
        },
        DiversityProfile {
            name: "maintainability_first".into(),
            traits: "Optimize for readability and future changes".into(),
        },
    ]),
    ..Default::default()
});
```

### Extended Debate Perspectives

```rust
let result = run_debate(RunDebateConfig {
    decision_question: "Which framework?".into(),
    perspectives: vec![
        "security".into(),
        "performance".into(),
        "simplicity".into(),
        "maintainability".into(),
        "cost".into(),
        "scalability".into(),
        "user_experience".into(),
    ],
    rounds: 4,  // More rounds for complex decisions
    ..Default::default()
});
```

## Contributing

When adding new patterns:

1. Create new file in `patterns/` directory
2. Implement using orchestration infrastructure
3. Add comprehensive doc comments
4. Update `mod.rs` exports
5. Add examples to `examples/patterns.rs`
6. Update this README

## References

- Orchestration Infrastructure: `../README.md`
- Workflows: `~/.amplihack/.claude/workflow/*.md`
- Core Philosophy: `~/.amplihack/.claude/context/PHILOSOPHY.md`
