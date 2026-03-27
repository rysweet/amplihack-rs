# Recipe Execution Flow

Illustrates how `amplihack-rs` loads, validates, and executes a recipe
step-by-step, including the hook dispatch lifecycle.

## Overview

A *recipe* is a YAML file describing a sequence of steps.  The Rust
runner resolves the recipe, launches a Claude session per step, streams
output, and records the result before moving to the next step.

## Execution Flow Diagram

```mermaid
flowchart TD
    A([User: amplihack run &lt;recipe&gt;]) --> B[Locate recipe file\nin .claude/recipes/]
    B --> C{Recipe found?}
    C -- No --> ERR1([Error: recipe not found])
    C -- Yes --> D[Parse & validate YAML]
    D --> E{Valid schema?}
    E -- No --> ERR2([Error: invalid recipe])
    E -- Yes --> F[Resolve $RECIPE_VAR_* variables\nfrom environment]
    F --> G[Build step execution plan\nstep list + dependencies]

    subgraph StepLoop ["Per-step execution loop"]
        H[Select next pending step] --> I[Emit pre-step hook\nStepStart event]
        I --> J[Spawn Claude session\nwith step prompt]
        J --> K[Stream output to TTY]
        K --> L{Step outcome}
        L -- success --> M[Emit post-step hook\nStepEnd event]
        L -- failure --> N{Retry policy?}
        N -- retry --> J
        N -- abort --> ERR3([Abort recipe])
        M --> O{More steps?}
        O -- yes --> H
        O -- no --> P([Recipe complete])
    end

    G --> H
```

## Key Design Decisions

| Decision | Rationale |
|---|---|
| Steps run sequentially by default | Deterministic output; easier to reason about |
| `$RECIPE_VAR_*` resolved before execution | Fail fast on missing variables |
| Pre/post step hooks | Allow Python agents to react without modifying the runner |
| Retry policy per step | Transient Claude failures should not abort long recipes |

## Related Concepts

- [Memory Backend Architecture](memory-backend-architecture.md)
- [Signal Handling Lifecycle](signal-handling-lifecycle.md)
- [Fleet State Machine](fleet-state-machine.md)
