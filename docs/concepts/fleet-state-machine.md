# Fleet State Machine

Documents the lifecycle of a *fleet* — a set of named Claude agent
sessions managed together by `amplihack fleet`.

## Session States

```mermaid
stateDiagram-v2
    [*] --> Pending : fleet start &lt;name&gt;

    Pending --> Launching : slot available
    Launching --> Running : tmux pane created\nClaude process started

    Running --> Idle : Claude waiting for input
    Idle --> Running : user sends prompt\nor recipe step dispatched

    Running --> Paused : SIGTSTP / fleet pause
    Paused --> Running : fleet resume

    Running --> Stopping : fleet stop &lt;name&gt;\nor recipe complete
    Idle --> Stopping : fleet stop &lt;name&gt;
    Stopping --> Stopped : process exited cleanly

    Running --> Crashed : non-zero exit\nor signal SIGKILL
    Crashed --> Pending : fleet restart &lt;name&gt;

    Stopped --> [*]
    Stopped --> Pending : fleet restart &lt;name&gt;
```

## Fleet Admiral State (Dashboard View)

```mermaid
stateDiagram-v2
    [*] --> Initialising : admiral start

    Initialising --> Polling : sessions registered
    Polling --> Rendering : tick interval elapsed
    Rendering --> Polling : frame drawn to TTY

    Polling --> Alerting : session entered Crashed state
    Alerting --> Polling : alert acknowledged

    Polling --> Shutting_Down : SIGINT / SIGTERM received
    Shutting_Down --> [*] : sessions drained\nor timeout
```

## Key Transitions

| Transition | Trigger | Side-Effect |
|---|---|---|
| Pending → Launching | Concurrency slot freed | tmux window created |
| Running → Crashed | Non-zero exit code | Alert emitted; metrics recorded |
| Crashed → Pending | `fleet restart` | Previous pane cleaned up |
| Running → Paused | `SIGTSTP` forwarded | Tmux pane kept alive |
| Polling → Shutting_Down | `SIGINT` (Ctrl-C) | Exit code 0 (SIGINT parity) |

## Related Concepts

- [Signal Handling Lifecycle](signal-handling-lifecycle.md)
- [Fleet Admiral Reasoning](fleet-admiral-reasoning.md)
- [Fleet Dashboard Architecture](fleet-dashboard-architecture.md)
