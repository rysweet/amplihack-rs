# Fleet Dashboard Architecture

This document explains how `amplihack fleet` is structured, why each design
decision was made, and what trade-offs were accepted.  Read this before
modifying `src/fleet.rs`.

## Contents

- [Overview](#overview)
- [Thread model](#thread-model)
- [State model](#state-model)
- [Key dispatch](#key-dispatch)
- [Session discovery](#session-discovery)
- [Persistence layer](#persistence-layer)
- [Terminal safety](#terminal-safety)
- [Security model](#security-model)
- [What was deliberately left out](#what-was-deliberately-left-out)

---

## Overview

The fleet dashboard is implemented entirely in `src/fleet.rs` as a set of
plain Rust structs and `std::thread` threads.  It does not use `tokio`,
`async`/`await`, or any TUI framework crate.  The render loop, the background
refresh threads, and the keyboard reader are all separate concerns connected
through `std::sync::mpsc` channels.

```
┌─────────────────────────────────────────────────┐
│                   main thread                   │
│  ┌───────────┐  ┌──────────────┐  ┌──────────┐ │
│  │  keyboard  │  │ render loop  │  │  state   │ │
│  │  reader   │→ │  (crossterm) │←─│ FleetTui │ │
│  └───────────┘  └──────────────┘  │ UiState  │ │
│        ↓                ↑         └──────────┘ │
│  DashboardKey      RefreshMsg                   │
└─────────────────────────────────────────────────┘
        ↑ mpsc::Receiver             ↑ mpsc::Sender
┌───────────────────┐    ┌────────────────────────┐
│  T4: fast refresh │    │  T5: slow refresh      │
│  (500 ms)         │    │  (5 s, tmux)            │
│  reads lock files │    │  reads capture-pane     │
└───────────────────┘    └────────────────────────┘
```

The entry point `run_fleet_dashboard(args, bg_tx)` accepts an
`Option<Sender<RefreshMsg>>`.  When `bg_tx` is `None` the function runs the
session-collect inline and returns immediately — this path is used in unit
tests to exercise all state transitions without spawning threads.

---

## Thread model

### T4 — fast refresh (500 ms)

Reads `~/.claude/runtime/locks/` on every tick and sends a
`RefreshMsg::Sessions(Vec<FleetSessionEntry>)` down the channel.  On a
channel-send error (receiver dropped) the thread calls `break` and exits
cleanly without panicking.

### T5 — slow refresh (5 s)

Calls `tmux capture-pane -t <session-id> -p` for each active session.  Sends
`SlowRefreshMsg::CaptureUpdate { session_id, content }`.  If `tmux` is absent
the thread immediately exits; the dashboard continues without preview content.

### Why `std::thread` instead of `tokio`?

The dashboard does two things that cooperate poorly with async: raw-mode
terminal I/O and blocking file system reads.  Dedicating OS threads to each
concern makes the code straightforward to read and avoids the overhead of an
async runtime for a use-case that is not network-bound.

---

## State model

All mutable render state lives in one struct:

```rust
pub struct FleetTuiUiState {
    pub selected_row: usize,
    pub scroll_offset: usize,
    pub active_panel: Panel,
    pub mode: DashboardMode,
    pub status_message: Option<String>,
}
```

| Field            | Purpose                                               |
| ---------------- | ----------------------------------------------------- |
| `selected_row`   | Which session row is highlighted                     |
| `scroll_offset`  | How many rows are scrolled off the top of the table  |
| `active_panel`   | `SessionTable`, `Editor`, `ProjectList`, or `Help`   |
| `mode`           | `Normal`, `Creating`, `Adopting`, or `Help`          |
| `status_message` | One-line message shown in the status bar; `None` = clear |

Having a single state struct makes snapshot-based unit testing straightforward:
construct an initial `FleetTuiUiState`, dispatch a `DashboardKey`, assert the
resulting state.

---

## Key dispatch

Raw terminal bytes are translated to a typed enum before they reach any
application logic:

```rust
pub enum DashboardKey {
    Up, Down, PageUp, PageDown,
    Tab, Enter, Escape,
    Char(char),
    CtrlC, CtrlU,
    Unknown,
}
```

The crossterm event loop maps `crossterm::event::KeyEvent` values to
`DashboardKey` variants.  All match arms on raw bytes are isolated to one
function (`key_from_event`), so the rest of the code never touches raw
terminal bytes.

---

## Session discovery

`collect_observed_fleet_state()` returns `Vec<FleetSessionEntry>`:

```rust
pub struct FleetSessionEntry {
    pub session_id: String,   // sanitized
    pub status: SessionStatus,
    pub pid: Option<u32>,
    pub project: Option<String>,
    pub age_secs: Option<u64>,
}

pub enum SessionStatus { Active, Idle, Dead, Unknown }
```

The function:

1. Reads `~/.claude/runtime/locks/*.lock` with `fs::read_dir`.
2. Calls `sanitize_session_id()` on every filename component before any use.
3. Parses each file as JSON; skips entries with parse errors.
4. Validates `pid` is in `1..=4_194_304`; marks entry `Dead` if out of range.
5. Checks whether the PID is live by reading `/proc/{pid}/stat` (Linux) or
   using `kill(pid, 0)` (macOS/BSD); sets `SessionStatus` accordingly.
6. Returns the collected vec sorted by age descending (newest first).

`sanitize_session_id` strips any byte outside `[a-zA-Z0-9_-]` and returns
`Err` on an empty result.

**Call-site coverage is mandatory at three use sites** — not just at read
time:

| Use site         | Risk if sanitization is skipped                                |
| ---------------- | -------------------------------------------------------------- |
| Map key          | Unsanitized ID used as a HashMap key can cause mismatched lookups if the same session appears under two forms |
| Display string   | Raw bytes reach the TUI renderer; malformed Unicode or ANSI escapes corrupt the terminal |
| Path component   | Unsanitized ID used in a file path enables path-traversal (`../`) attacks |

Any new code that consumes a session ID from a lock file must call
`sanitize_session_id()` before using the result in any of these three
contexts.

---

## Persistence layer

`FleetDashboardSummary` is the single persisted struct:

```rust
#[derive(Serialize, Deserialize, Default)]
pub struct FleetDashboardSummary {
    #[serde(default)]
    pub version: u8,
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    #[serde(default)]
    pub last_full_refresh: Option<i64>,
    #[serde(default)]
    pub extras: HashMap<String, serde_json::Value>,
}
```

`#[serde(default)]` on every field means any older file missing a field
deserializes without error.  Unknown top-level fields land in `extras` and are
preserved on the next write, providing forward compatibility.

### Atomic write sequence

1. Serialize to JSON bytes.
2. Open a temp file in the **same directory** as `fleet_dashboard.json` (not
   `/tmp`; same-directory guarantees the rename is on the same filesystem mount
   point).
3. Set Unix permissions `0600` before writing any bytes.
4. Write all bytes.
5. `fsync` the temp file (flush kernel buffers to disk).
6. `rename(temp, target)` — atomic on POSIX.

### Capture cache

`FleetCaptureCache` is an in-memory LRU:

- Backed by a `VecDeque<(String, String)>` (session_id → content).
- Capacity: 64 entries.  When full, the oldest entry is evicted before
  inserting a new one.
- Each entry is capped at 64 KiB; content exceeding this is truncated before
  insertion.
- Any field holding this cache in a `Serialize` struct must be annotated
  `#[serde(skip)]` to prevent accidental serialization of ephemeral terminal
  content.

---

## Terminal safety

### RAII terminal guard

`crossterm::terminal::enable_raw_mode()` is called once at dashboard startup.
The return value is wrapped in a `TerminalGuard` whose `Drop` impl calls
`crossterm::terminal::disable_raw_mode()`:

```rust
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen,
        );
    }
}
```

Because `drop` runs even when the stack unwinds (Rust panics), the terminal is
always restored — the user's shell is never left in raw mode.

### OSC escape stripping

`tmux capture-pane` output can contain OSC sequences (e.g., terminal
hyperlinks or colour-setting sequences) embedded by the captured program.
Before any captured content reaches the render loop, `strip_osc_sequences()`
removes both OSC termination forms defined by ECMA-48:

- `\x1b] … \x07`  (BEL-terminated OSC — the older, widely-used form)
- `\x1b] … \x1b\\`  (ST-terminated OSC — the standards-compliant form)

Both forms must be stripped.  Stripping only the BEL form leaves an
injection vector via ST-terminated sequences; stripping only ST leaves the
BEL vector open.  Any OSC strip function that handles only one form is
incomplete.

This prevents terminal-injection attacks through captured pane content.

### Platform guard for PID-reuse check

The PID-reuse guard uses different mechanisms per platform, controlled by
compile-time conditional compilation:

```rust
#[cfg(target_os = "linux")]
fn verify_comm(pid: u32, expected: &str) -> bool {
    // reads /proc/{pid}/comm
}

#[cfg(not(target_os = "linux"))]
fn verify_comm(pid: u32, expected: &str) -> bool {
    // uses sysctl CTL_KERN / KERN_PROC / KERN_PROC_PID on macOS/BSD
}
```

On **Linux** the check reads `/proc/{pid}/comm` — a single-read, low-overhead
operation.  On **macOS** (and other BSD-derived systems) the check uses the
`sysctl` API to query the process command name.  Both paths must agree with
the expected Claude process name before a signal is sent.

Users on macOS will observe the same _behaviour_ (adoption blocked for
mismatched processes) but the underlying mechanism differs.  If a macOS user
sees unexpected adoption failures they should check whether the `sysctl`
API is available in their sandbox environment.

---

## Security model

| Concern                 | Mitigation                                                  |
| ----------------------- | ----------------------------------------------------------- |
| Path traversal via session IDs | `sanitize_session_id()` on every lock-file name  |
| Symlink attacks on project paths | `canonicalize()` before `is_dir()` check      |
| PID reuse (signal to wrong process) | UID check + `/proc/{pid}/comm` (or `sysctl`) before any signal |
| Oversized tmux output exhausting memory | 64 KiB per-entry cap in `FleetCaptureCache` |
| Sensitive data in serialized state | `#[serde(skip)]` on capture cache fields     |
| Partial writes to fleet config | Atomic rename sequence (temp file same dir)     |
| Terminal injection via captured content | OSC sequence stripping before render       |
| Leaked paths in TUI error messages | `FleetError::Display` shows category only; `Debug` shows detail |

`FleetError` has 10 variants.  The `Display` impl deliberately omits raw
filesystem paths, PIDs, and internal state — these appear only in the `Debug`
representation, which is written to log files rather than shown in the TUI.

---

## What was deliberately left out

### No `tokio`

The refresh loop does two blocking sleeps per thread and a directory read.
None of that benefits from cooperative scheduling.  Adding `tokio` would
increase compile time and binary size with no measurable gain.

### No TUI framework crate (`ratatui`, `tui-rs`, etc.)

The cockpit renderer is a hand-written ANSI renderer using `crossterm`
primitives.  This keeps the dependency surface minimal and makes it easy to
audit what terminal escapes are being sent.  A framework crate would be
justified if the widget count grew significantly.

### No persistent session-content cache

`FleetCaptureCache` is in-memory only.  Writing captured terminal content to
disk would require careful scrubbing of secrets (API keys, tokens) that may
appear in Claude's output.  The per-session 64 KiB cap already limits memory
use to ~4 MiB for a full 64-entry cache.

### Conditional workspace helpers: `sanitize_session_id` and `AtomicJsonFile`

> **For contributors** — Both `sanitize_session_id()` and `AtomicJsonFile`
> may not exist as standalone items in the Rust workspace depending on the
> build configuration.  If you find them absent, consult the inline fallback
> rules documented in the spec (RISK-02 for sanitization, RISK-03 for atomic
> writes).  Do not assume their absence means the behaviour is unimplemented —
> the logic may be inlined at the call site.

### No `v0.4.x` version tag

Version numbers jump from `v0.3.x` directly to `v0.5.0` to avoid a collision
with tags on the main amploxy branch that used `v0.4.x` during the parallel
development period.  This is documented in `CHANGELOG.md`.

---

**See also**

- [How to Use the Fleet Dashboard](../howto/use-fleet-dashboard.md)
- [amplihack fleet — CLI reference](../reference/fleet-command.md)
- [Signal Handling and Exit Codes](../reference/signal-handling.md)
