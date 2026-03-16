# amplihack fleet — CLI Reference

Full reference for the `fleet` subcommand introduced in **v0.5.0**.

## Prerequisites

| Dependency | Required? | Effect when absent                                             |
| ---------- | --------- | -------------------------------------------------------------- |
| `tmux`     | Optional  | Capture-pane preview disabled; slow-refresh thread exits silently. Dashboard remains fully functional — session table, project management, and new-session launch all work. |

## Synopsis

```
amplihack fleet [OPTIONS]
```

## Description

`amplihack fleet` opens a full-screen terminal dashboard that displays all
active Claude sessions on the local machine, lets you launch new sessions,
adopt existing ones, and manage a persistent list of tracked projects.

The command requires a real TTY.  It exits immediately with code 0 when
`--help` is passed, making it safe to call in existence checks.

## Options

| Flag              | Type    | Default | Description                                       |
| ----------------- | ------- | ------- | ------------------------------------------------- |
| `--no-color`      | bool    | false   | Disable ANSI colour output in the session table   |
| `--refresh-fast`  | ms      | 500     | Poll interval for the session-list refresh thread |
| `--refresh-slow`  | ms      | 5000    | Poll interval for the tmux capture-pane thread    |
| `-h`, `--help`    | —       | —       | Print help and exit 0                             |

## Key Bindings

### Navigation

| Key            | Action                                     |
| -------------- | ------------------------------------------ |
| `↑` / `↓`     | Move selection up / down in session table  |
| `PgUp` / `PgDn` | Scroll session table by one screen        |
| `Tab`          | Switch focus between panels                |

### Session management

| Key | Action                                                             |
| --- | ------------------------------------------------------------------ |
| `n` | Open inline editor to start a new Claude session                  |
| `a` | Adopt (attach to) the currently selected session                  |
| `k` | Send SIGTERM to the selected session (requires UID + comm checks) |

### Project management

| Key | Action                                                  |
| --- | ------------------------------------------------------- |
| `p` | Prompt for a project path to add to the tracked list   |
| `P` | Remove the selected project from the tracked list      |

### Editor (when editor panel is active)

| Key             | Action                                         |
| --------------- | ---------------------------------------------- |
| `Enter`         | Submit prompt / confirm action                 |
| `Esc`           | Cancel editor and return to session table      |
| `Tab`           | Apply AI-suggested continuation at cursor      |
| `←` / `→`      | Move cursor left / right                       |
| `↑` / `↓`      | Move cursor to previous / next line            |
| `Home` / `End`  | Jump to start / end of current line            |
| `Ctrl-U`        | Clear current line                             |

### Global

| Key      | Action                                                           |
| -------- | ---------------------------------------------------------------- |
| `q`      | Exit fleet dashboard and restore terminal                        |
| `Ctrl-C` | Exit fleet dashboard and restore terminal (identical to `q`)    |
| `?`      | Toggle in-dashboard help overlay                                 |

## Exit Codes

| Code | Meaning                                      |
| ---- | -------------------------------------------- |
| 0    | Normal exit (user pressed `q` or `Ctrl-C`)  |
| 1    | Terminal could not be opened (no TTY)        |
| 2    | I/O error reading lock files                 |

## Persistent State

`amplihack fleet` reads and writes one JSON file:

```
~/.claude/runtime/fleet_dashboard.json
```

The file is created on first run with Unix permissions `0600`.  It is updated
atomically: the binary writes to a temp file in the same directory and then
calls `rename(2)` so partial writes are never visible.

### fleet_dashboard.json schema

```json
{
  "version": 1,
  "projects": [
    "/home/alice/src/amplihack-rs",
    "/home/alice/src/my-project"
  ],
  "last_full_refresh": 1741872000,
  "extras": {}
}
```

| Field               | Type              | Description                                           |
| ------------------- | ----------------- | ----------------------------------------------------- |
| `version`           | `u8`              | Schema version; currently `1`                        |
| `projects`          | `[string]`        | Canonicalized absolute paths of tracked project dirs |
| `last_full_refresh` | `i64` or `null`   | Unix timestamp of the last complete session scan     |
| `extras`            | `object`          | Reserved for forward-compatible extension fields     |

Unknown fields in `extras` are preserved on round-trip.  Unknown top-level
fields are ignored (forward-compatible reads).

> **Capture cache is not persisted.** The in-memory session-capture cache
> (`FleetCaptureCache`) is never written to this file.  Any captured terminal
> content is ephemeral and lost when the dashboard exits.

### File permissions

`fleet_dashboard.json` is created with mode `0600` (owner read/write only).
**Do not change this permission.**  Relaxing it (e.g., `chmod 644`) exposes
your tracked project paths to other local users and weakens the security
posture of the dashboard's atomic-write sequence.

## Session Lock Files

The dashboard discovers sessions by reading:

```
~/.claude/runtime/locks/<session-id>.lock
```

Each lock file is a JSON object with at least the following fields:

```json
{
  "pid": 12345,
  "project": "/home/alice/src/amplihack-rs",
  "started_at": 1741871900
}
```

Every `<session-id>` component is passed through `sanitize_session_id()` before
use as a map key, display string, or file-path component.  Sanitization strips
any character that is not `[a-zA-Z0-9_-]` and rejects the empty result.

## Refresh Architecture

Two background threads run independently of the render loop:

| Thread | Interval | Source                       | Message type                    | Shutdown                                       |
| ------ | -------- | ---------------------------- | ------------------------------- | ---------------------------------------------- |
| T4     | 500 ms   | `~/.claude/runtime/locks/`   | `RefreshMsg::Sessions`          | Exits when receiver is dropped (`send()` → `Err`) |
| T5     | 5 s      | `tmux capture-pane`          | `SlowRefreshMsg::CaptureUpdate` | Exits on channel close, or immediately if `tmux` is absent |

Neither thread blocks the keyboard input loop.  Both self-exit without
panicking when the main thread drops the `mpsc` receiver — the `send()`
call returns `Err` and the thread calls `break`.

## Runtime Limits

| Resource                | Limit              | Behaviour when exceeded         |
| ----------------------- | ------------------ | -------------------------------- |
| Capture cache entries   | 64                 | Oldest entry evicted before insert |
| Capture cache entry size | 64 KiB            | Content truncated before insert |
| Editor characters/line  | 4 096              | Further input silently dropped  |
| Editor lines            | 200                | Further lines silently dropped  |
| Prompt handoff cap      | 1 000 characters   | Truncated before session launch |

All limits are security caps enforced silently; no error is raised when
content exceeds a limit.

## Error Reporting

`FleetError` has two representation forms:

| Form      | Content                                   | Where it appears          |
| --------- | ----------------------------------------- | ------------------------- |
| `Display` | Error category only (e.g., "permission denied", "invalid session") | TUI status bar |
| `Debug`   | Full detail including paths, PIDs, and internal state | Log files at `~/.claude/runtime/logs/` |

The `Display` impl **deliberately omits** raw filesystem paths, process IDs,
and internal state from TUI messages.  This prevents sensitive path or PID
information from being visible on shared screens.  Full diagnostic detail is
always available in the log files.

## Security Properties

| Property                | Detail                                                               |
| ----------------------- | -------------------------------------------------------------------- |
| Session ID sanitization | `sanitize_session_id()` called on every ID before any use           |
| Path canonicalization   | `std::fs::canonicalize()` called on every user-supplied project path |
| PID validation          | Accepted range `1..=4_194_304`; integers outside this are rejected  |
| UID check               | Adopted PID must be owned by the current user's UID                 |
| comm check              | `/proc/{pid}/comm` (Linux) or `sysctl` (macOS) checked before signal|
| OSC stripping           | `\x1b]…\x07` and `\x1b]…\x1b\\` sequences stripped from tmux output|
| File permissions        | `fleet_dashboard.json` created with mode `0600`                     |
| Atomic writes           | Temp-file-then-rename; temp file in same directory as target        |
| Editor sanitization     | Control bytes `< 0x20` (except `\t`, `\n`) stripped before handoff |
| Capture cache cap       | Each cache entry capped at 64 KiB; excess discarded silently        |

## Environment Variables

| Variable                 | Effect                                                 |
| ------------------------ | ------------------------------------------------------ |
| `NO_COLOR`               | Disable ANSI colours (same effect as `--no-color`)    |
| `AMPLIHACK_FLEET_FAST_MS`| Override fast-refresh interval (milliseconds)          |
| `AMPLIHACK_FLEET_SLOW_MS`| Override slow-refresh interval (milliseconds)          |

## Related

- [How to Use the Fleet Dashboard](../howto/use-fleet-dashboard.md)
- [Fleet Dashboard Architecture](../concepts/fleet-dashboard-architecture.md)
- [Signal Handling and Exit Codes](../reference/signal-handling.md)
- [Environment Variables](../reference/environment-variables.md)
