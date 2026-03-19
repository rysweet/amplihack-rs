# Signal Handling Lifecycle

Documents how `amplihack-rs` handles OS signals (SIGINT, SIGTERM, SIGTSTP,
SIGWINCH) across the CLI, fleet manager, and child processes.

## Signal Dispatch Overview

```mermaid
flowchart TD
    OS([OS sends signal]) --> Router{Which process\nreceives it?}

    Router -- amplihack CLI\nforeground --> CLIHandler[CLI signal handler]
    Router -- tmux pane\nchild process --> TmuxPropagation[tmux propagates\nto Claude process]

    subgraph CLIHandler
        SigInt[SIGINT\nCtrl-C] --> GracefulShutdown[Drain in-flight ops\nexit code 0]
        SigTerm[SIGTERM] --> GracefulShutdown
        SigTstp[SIGTSTP\nCtrl-Z] --> PauseFleet[Pause all Running\nfleet sessions]
        SigWinch[SIGWINCH\nterminal resize] --> Repaint[Recalculate layout\nredraw TUI]
    end

    GracefulShutdown --> Exit0([Exit 0])
    PauseFleet --> Suspend([Process suspended])
    Repaint --> Dashboard([Dashboard redrawn])
```

## SIGINT Parity Contract

SIGINT (Ctrl-C) **must exit with code 0**, not 130.  This matches the
Python amplihack behaviour and allows shell scripts to treat user
interruption as a normal exit.

```mermaid
sequenceDiagram
    participant User
    participant CLI as amplihack CLI
    participant Fleet as Fleet Manager
    participant Claude as Claude Process

    User->>CLI: Ctrl-C (SIGINT)
    CLI->>Fleet: signal: shutdown
    Fleet->>Claude: SIGTERM (graceful)
    Claude-->>Fleet: exited
    Fleet-->>CLI: all sessions stopped
    CLI->>CLI: flush metrics / memory
    CLI-->>User: exit 0  ✅
```

## Signal Forwarding to Child Sessions

```mermaid
flowchart LR
    SIGINT --> |"fleet admiral\nreceives"| Admiral
    Admiral --> |"for each Running session"| TmuxSend["tmux send-keys\n'' Enter"]
    TmuxSend --> ClaudeExit[Claude exits cleanly]
    ClaudeExit --> SessionState[Session → Stopped]
    SessionState --> Admiral
    Admiral --> |"all sessions drained"| Exit0([exit 0])
```

## Terminal Resize (SIGWINCH)

```mermaid
sequenceDiagram
    participant Terminal
    participant CLI as amplihack TUI
    participant Renderer

    Terminal->>CLI: SIGWINCH (terminal resized)
    CLI->>CLI: query new dimensions\n(ioctl TIOCGWINSZ)
    CLI->>Renderer: layout(new_width, new_height)
    Renderer->>Terminal: redraw all panels
```

## Related Concepts

- [Fleet State Machine](fleet-state-machine.md)
- [Fleet Admiral Reasoning](fleet-admiral-reasoning.md)
- [Recipe Execution Flow](recipe-execution-flow.md)
