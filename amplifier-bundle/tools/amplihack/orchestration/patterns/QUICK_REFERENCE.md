# Pattern Orchestrators Quick Reference

## Import

```rust
use amplihack_orchestration::patterns::{
    run_n_version,
    run_debate,
    run_cascade,
    create_custom_cascade,
};
```

## N-Version Programming

**Use when:** Critical implementations requiring multiple attempts

```rust
let result = run_n_version(NVersionConfig {
    task_prompt: "Implement feature X",
    n: 3,                                // Number of versions
    selection_criteria: Some(vec![       // Optional
        "correctness",
        "security",
        "simplicity",
    ]),
    timeout: Some(Duration::from_secs(300)), // Per version
    ..Default::default()
})?;

// Access results
println!("{}", result.selected);         // "version_1", "version_2", etc.
println!("{}", result.rationale);        // Why selected
println!("{:?}", result.versions);       // All ProcessResult objects
println!("{}", result.comparison);       // Reviewer analysis
```

## Multi-Agent Debate

**Use when:** Complex decisions needing multiple viewpoints

```rust
let result = run_debate(DebateConfig {
    decision_question: "Which database?",
    perspectives: Some(vec![             // Optional
        "security",
        "performance",
        "simplicity",
    ]),
    rounds: 3,                           // Number of debate rounds
    timeout: Some(Duration::from_secs(180)), // Per perspective
    ..Default::default()
})?;

// Access results
println!("{}", result.synthesis);        // Final consensus
println!("{}", result.confidence);       // "HIGH", "MEDIUM", "LOW"
println!("{:?}", result.rounds);         // Each round's results
println!("{:?}", result.positions);      // History per perspective
```

## Fallback Cascade

**Use when:** Need guaranteed completion with degradation

```rust
let result = run_cascade(CascadeConfig {
    task_prompt: "Generate documentation",
    fallback_strategy: FallbackStrategy::Quality, // Quality, Service, Freshness, Completeness, Accuracy
    timeout_strategy: TimeoutStrategy::Balanced,   // Aggressive, Balanced, Patient
    notification_level: NotificationLevel::Warning, // Silent, Warning, Explicit
    ..Default::default()
})?;

// Access results
println!("{}", result.cascade_level);    // "primary", "secondary", "tertiary"
println!("{}", result.degradation);      // Degradation description
println!("{:?}", result.result);         // Final ProcessResult
println!("{:?}", result.attempts);       // All attempts
```

## Custom Cascade

**Use when:** Need specific cascade behavior

```rust
let result = create_custom_cascade(CustomCascadeConfig {
    task_prompt: "Analyze code",
    levels: vec![
        CascadeLevel {
            name: "comprehensive",
            timeout: Duration::from_secs(120),
            constraint: "full analysis",
            model: None,           // Optional
        },
        CascadeLevel {
            name: "quick",
            timeout: Duration::from_secs(30),
            constraint: "basic analysis",
            model: None,
        },
        CascadeLevel {
            name: "minimal",
            timeout: Duration::from_secs(5),
            constraint: "syntax check only",
            model: None,
        },
    ],
    ..Default::default()
})?;
```

## Common Parameters

All patterns support:

| Parameter     | Type              | Default     | Description                       |
| ------------- | ----------------- | ----------- | --------------------------------- |
| `working_dir` | `PathBuf`         | current dir | Working directory                 |
| `model`       | `Option<String>`  | None        | Claude model (None = CLI default) |
| `timeout`     | `Option<Duration>`| None        | Timeout per process               |

## Return Structure

All patterns return a `PatternResult` struct with:

| Field                               | Type     | Description                 |
| ----------------------------------- | -------- | --------------------------- |
| `session_id`                        | `String` | Session identifier for logs |
| `success`                           | `bool`   | Whether operation succeeded |
| Additional fields specific to pattern |        | See pattern docs            |

## Session Logs

Find logs at: `~/.amplihack/.claude/runtime/logs/<session_id>/`

- `session.log` - Session overview
- `<process_id>.log` - Individual process logs

## Error Handling

All patterns handle errors gracefully:

```rust
let result = run_n_version(config)?;
if !result.success {
    eprintln!("Failed: {}", result.rationale);
    // Check individual attempts
    for version in &result.versions {
        if version.exit_code != 0 {
            eprintln!("Error: {}", version.stderr);
        }
    }
}
```

## Timeout Strategies

For `run_cascade()`:

| Strategy     | Primary | Secondary | Tertiary |
| ------------ | ------- | --------- | -------- |
| `aggressive` | 5s      | 2s        | 1s       |
| `balanced`   | 30s     | 10s       | 5s       |
| `patient`    | 120s    | 30s       | 10s      |

## Fallback Strategies

For `run_cascade()`:

| Strategy       | Primary       | Secondary   | Tertiary   |
| -------------- | ------------- | ----------- | ---------- |
| `quality`      | comprehensive | standard    | minimal    |
| `service`      | optimal API   | cached      | defaults   |
| `freshness`    | real-time     | recent      | historical |
| `completeness` | full dataset  | sample      | summary    |
| `accuracy`     | precise       | approximate | estimate   |

## Perspective Profiles

For `run_debate()`:

| Perspective       | Focus                       | Questions                   |
| ----------------- | --------------------------- | --------------------------- |
| `security`        | Vulnerabilities, protection | What could go wrong?        |
| `performance`     | Speed, scalability          | Will this scale?            |
| `simplicity`      | Minimal complexity          | Is this the simplest?       |
| `maintainability` | Long-term evolution         | Can future devs understand? |
| `user_experience` | API design, usability       | Is this intuitive?          |

## Diversity Profiles

For `run_n_version()`:

| Profile               | Approach                |
| --------------------- | ----------------------- |
| `conservative`        | Proven patterns, safety |
| `pragmatic`           | Balanced trade-offs     |
| `minimalist`          | Ruthless simplicity     |
| `innovative`          | Novel approaches        |
| `performance_focused` | Speed optimization      |

## Pattern Selection

Quick decision guide:

```
Need multiple implementations?
├─ Yes → N-Version Programming
└─ No
   ├─ Need to make decision?
   │  ├─ Yes → Multi-Agent Debate
   │  └─ No → Continue
   └─ Need guaranteed completion?
      ├─ Yes → Fallback Cascade
      └─ No → Use standard execution
```

## Examples

See `examples/patterns.rs` for complete working examples.

## Full Documentation

See `README.md` for comprehensive documentation.
