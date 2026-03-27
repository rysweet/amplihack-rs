# amplihack fleet — CLI Reference

Full reference for the `fleet` subcommand introduced in **v0.5.0**.

`amplihack fleet` has two operating modes:

| Mode | How to invoke | Purpose |
|------|---------------|---------|
| **Local dashboard** | `amplihack fleet` (no subcommand) | Interactive TUI for sessions on the local machine |
| **Azure VM orchestration** | `amplihack fleet <subcommand>` | Discover, reason about, and act on sessions across Azure VMs via `azlin` |

The local dashboard requires a real TTY.  The Azure VM subcommands
(`scout`, `advance`, `dry-run`, `start`) run non-interactively and are
safe to call from scripts and CI pipelines.

> **Local mode and process restart** — The local dashboard tracks UI state
> (selected row, active filters, editor contents, proposal notices) using
> `AtomicBool` and `Arc<Mutex<…>>` in-memory flags.  **None of this state
> survives process exit.**  Only the `fleet_dashboard.json` file (tracked
> project list) is persisted to disk.  If the dashboard exits unexpectedly,
> the project list is preserved but any in-progress editor content or
> uncommitted proposals are lost.

## Prerequisites

| Dependency | Required? | Effect when absent                                             |
| ---------- | --------- | -------------------------------------------------------------- |
| `tmux`     | Optional  | Capture-pane preview disabled; slow-refresh thread exits silently. Dashboard remains fully functional — session table, project management, and new-session launch all work. |

## Synopsis

```
amplihack fleet [OPTIONS]
amplihack fleet <SUBCOMMAND> [SUBCOMMAND-OPTIONS]
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

## Azure VM Subcommands

All Azure VM subcommands require `azlin` to be installed and reachable.  Set
`AZLIN_PATH` if `azlin` is not on your `PATH`.

### `fleet dry-run`

Analyse sessions on one or more VMs without taking any action.  Prints the
fleet admiral reasoning report and exits.

```
amplihack fleet dry-run [--vm <VM>]... [--priorities <TEXT>] [--backend <BACKEND>]
```

| Flag            | Type     | Default  | Description                                               |
| --------------- | -------- | -------- | --------------------------------------------------------- |
| `--vm`          | string   | (all)    | Restrict discovery to this VM. Repeat for multiple VMs.   |
| `--priorities`  | string   | (none)   | Free-text priority guidance passed to the reasoner        |
| `--backend`     | string   | `auto`   | Reasoner backend: `auto`, `claude`, or `none`             |

**Example output**

```
Fleet Admiral Dry Run -- 3 sessions analyzed

Summary:
  send_input: 2
  wait: 1

amplihack-vm-01/work-session-1 [running] -> send_input (87%)
  Reason: Tests failing; agent needs a nudge to retry.
  Input: "Run the failing tests again and fix any new errors."

amplihack-vm-01/work-session-2 [idle] -> wait (70%)
  Reason: Session appears idle awaiting external review.

amplihack-vm-02/work-session-3 [running] -> send_input (82%)
  Reason: PR is ready; agent should push and open it.
  Input: "Push your branch and open a pull request now."
```

---

### `fleet scout`

A three-phase operation: discover sessions → adopt them into the task queue →
reason about each one with the LLM backend → print a scout report.

```
amplihack fleet scout [--vm <VM>] [--session <SESSION>] [--skip-adopt] [--incremental] [--save <PATH>]
```

| Flag            | Type   | Default | Description                                                         |
| --------------- | ------ | ------- | ------------------------------------------------------------------- |
| `--vm`          | string | (all)   | Restrict to sessions on this VM                                     |
| `--session`     | string | (all)   | Restrict to a single session name                                   |
| `--skip-adopt`  | bool   | false   | Skip Phase 2 adoption; report on already-adopted sessions only      |
| `--incremental` | bool   | false   | Skip sessions whose status hasn't changed since the last scout run  |
| `--save`        | path   | (none)  | Write the full JSON scout report to this path in addition to stdout |

**Phases**

| Phase | Label in output              | What it does                                                    |
| ----- | ---------------------------- | --------------------------------------------------------------- |
| 1     | `Discovering fleet sessions` | Calls `azlin list` to enumerate running VMs and tmux sessions   |
| 2     | `Adopting sessions`          | Registers discovered sessions in the local task queue           |
| 3     | `Reasoning about sessions`   | Calls the LLM reasoner for each session; prints the scout report |

**Example output**

```
Phase 1: Discovering fleet sessions...
Phase 2: Adopting sessions...
  amplihack-vm-01: adopted 2 sessions
Total adopted: 2
Phase 3: Reasoning about sessions...
  Reasoning: amplihack-vm-01/work-session-1...
  Reasoning: amplihack-vm-01/work-session-2...

============================================================
FLEET SCOUT REPORT
============================================================
VMs discovered: 2
Running VMs: 1
Sessions analyzed: 2
Adopted sessions: 2
Actions:
  send_input: 1
  wait: 1

  amplihack-vm-01/work-session-1 [running] -> send_input (87%)
    Branch: feature/auth-refactor
    PR: https://github.com/org/repo/pull/42
    Project: amplihack-rs
    Reason: Agent is idle; tests are passing; PR should be opened.
    Input: "Open a pull request for your current branch."

  amplihack-vm-01/work-session-2 [idle] -> wait (70%)
    Branch: main
    Reason: No active task; waiting for assignment.
```

**Incremental mode**

On the second and later runs, pass `--incremental` to skip sessions whose
status matches the cached result from `~/.claude/runtime/fleet/last_scout.json`:

```sh
amplihack fleet scout --incremental
# Incremental mode: loaded 3 previous statuses
#   Skipping (unchanged): amplihack-vm-01/work-session-2 [idle]
```

---

### `fleet advance`

Reason about all sessions and **execute** the recommended actions, with
interactive confirmation for destructive operations.

```
amplihack fleet advance [--vm <VM>] [--session <SESSION>] [--force] [--save <PATH>]
```

| Flag        | Type   | Default | Description                                                     |
| ----------- | ------ | ------- | --------------------------------------------------------------- |
| `--vm`      | string | (all)   | Restrict to sessions on this VM                                 |
| `--session` | string | (all)   | Restrict to a single session name                               |
| `--force`   | bool   | false   | Skip interactive confirmation prompts (suitable for automation) |
| `--save`    | path   | (none)  | Write the full JSON advance report to this path                 |

**Confirmation behaviour**

Without `--force`, `advance` prompts before executing `send_input` (default
`Y`) and `restart` (default `N`) actions.  `wait`, `escalate`, and
`mark_complete` are no-ops and never prompt.

```
    -> send_input: "Run the tests." (conf=87%) Execute? [Y/n]
```

**Example output**

```
============================================================
FLEET ADVANCE REPORT
============================================================
Sessions analyzed: 3
  send_input: 2
  wait: 1

  [OK] amplihack-vm-01/work-session-1 -> send_input
  [SKIPPED] amplihack-vm-01/work-session-2 -> wait
  [ERROR] amplihack-vm-02/work-session-3 -> send_input: tmux send-keys failed
```

**Exit codes** — `advance` exits `0` even when individual sessions fail.  Check
the `[ERROR]` lines in the report or inspect the JSON output to identify
failures.

---

### `fleet start`

Run the full fleet admiral orchestration loop.  The admiral repeatedly
discovers sessions, reasons about them, and executes recommended actions until
`Ctrl-C` or `--max-cycles` is reached.

```
amplihack fleet start [--max-cycles <N>] [--interval <SECS>] [--adopt] [--stuck-threshold <SECS>] [--max-agents-per-vm <N>] [--capture-lines <N>]
```

| Flag                 | Type   | Default | Description                                                         |
| -------------------- | ------ | ------- | ------------------------------------------------------------------- |
| `--max-cycles`       | u32    | 0       | Stop after N cycles. `0` means run indefinitely until `Ctrl-C`.    |
| `--interval`         | secs   | 60      | Seconds to sleep between cycles                                     |
| `--adopt`            | bool   | false   | Adopt all existing sessions on startup before the first cycle       |
| `--stuck-threshold`  | secs   | 300     | Seconds of no-output before a session is classified as `Stuck`      |
| `--max-agents-per-vm`| usize  | 3       | Maximum simultaneous sessions the admiral will manage per VM        |
| `--capture-lines`    | usize  | 50      | Lines of tmux output to capture per session during reasoning        |

**Example**

```sh
# Run for 10 cycles, adopting existing sessions first:
amplihack fleet start --max-cycles 10 --adopt

# Run indefinitely, polling every 2 minutes:
amplihack fleet start --interval 120
```

Set `AMPLIHACK_FLEET_EXISTING_VMS` (comma-separated VM names) to tell the
admiral which VMs already existed before this run, so it excludes them from
newly-allocated session slots.

---

### `fleet run-once`

Execute exactly one admiral cycle and exit.  Useful for testing and debugging
the orchestration logic without running a full loop.

```
amplihack fleet run-once
```

Output:

```
Cycle completed: 2 actions taken
  send_input: Sent nudge to work-session-1
  send_input: Sent PR-open prompt to work-session-3
```

---

### `fleet auth <VM_NAME>`

Authenticate the specified VM against GitHub, Azure, and Claude services.

```
amplihack fleet auth <VM_NAME> [--services github,azure,claude]
```

| Arg         | Type     | Default               | Description                               |
| ----------- | -------- | --------------------- | ----------------------------------------- |
| `VM_NAME`   | string   | (required)            | The `azlin`-registered VM to authenticate |
| `--services`| string[] | `github,azure,claude` | Comma-separated list of services          |

---

### `fleet adopt <VM_NAME>`

Manually adopt named sessions from a VM into the task queue without running the
full scout reasoning phase.

```
amplihack fleet adopt <VM_NAME> [--sessions <SESSION>]...
```

---

### `fleet observe <VM_NAME>`

Print a live tail of all session captures from the specified VM.  Exits when
`Ctrl-C` is pressed.

```
amplihack fleet observe <VM_NAME>
```

---

### `fleet watch <VM_NAME> <SESSION_NAME>`

Stream the terminal output from a single named session.

```
amplihack fleet watch <VM_NAME> <SESSION_NAME> [--lines <N>]
```

| Arg          | Type   | Default | Description                              |
| ------------ | ------ | ------- | ---------------------------------------- |
| `VM_NAME`    | string | —       | VM hosting the session                   |
| `SESSION_NAME`| string | —      | Tmux session name to watch               |
| `--lines`    | u32    | 30      | Number of terminal lines to capture      |

---

### `fleet add-task`

Enqueue a new task into the fleet task queue for dispatch by the admiral.

```
amplihack fleet add-task <PROMPT> [--repo <URL>] [--priority high|medium|low] [--agent claude|copilot|codex] [--mode auto|code|ask] [--max-turns <N>] [--protected]
```

| Flag          | Type   | Default    | Description                                                  |
| ------------- | ------ | ---------- | ------------------------------------------------------------ |
| `PROMPT`      | string | (required) | Task description given to the agent                          |
| `--repo`      | URL    | (none)     | Repository to clone on the VM before starting the session    |
| `--priority`  | enum   | `medium`   | Queue priority: `high`, `medium`, or `low`                   |
| `--agent`     | enum   | `claude`   | Agent binary: `claude`, `copilot`, or `codex`                |
| `--mode`      | enum   | `auto`     | Launch mode: `auto`, `code`, or `ask`                        |
| `--max-turns` | u32    | 20         | Maximum agent turns before the admiral considers escalation   |
| `--protected` | bool   | false      | Mark task as protected (admiral will not auto-restart it)    |

**Example**

```sh
amplihack fleet add-task \
  "Fix the failing CI tests in the auth module" \
  --repo https://github.com/org/amplihack-rs \
  --priority high \
  --agent claude \
  --max-turns 30
```

---

### `fleet setup`

Initialise the fleet home directory (`~/.claude/runtime/fleet/`) and create the
default task queue file.  Safe to run repeatedly (idempotent).

```
amplihack fleet setup
```

---

### `fleet status`

Print a one-line summary of the current fleet state: number of running VMs,
active sessions, and pending tasks.

```
amplihack fleet status
```

---

### `fleet snapshot`

Write the current fleet state to `~/.claude/runtime/fleet/snapshot.json`.
Useful before a maintenance window or before running `start` for the first time.

```
amplihack fleet snapshot
```

---

### `fleet report`

Print the contents of `~/.claude/runtime/fleet/last_scout.json` in a
human-readable form.

```
amplihack fleet report
```

---

### `fleet queue`

Print the current task queue, including pending, running, and completed tasks.

```
amplihack fleet queue
```

---

## Key Bindings

### Navigation

| Key            | Action                                     |
| -------------- | ------------------------------------------ |
| `↑` / `↓`     | Move selection up / down in session table  |
| `PgUp` / `PgDn` | Scroll session table by one screen        |
| `Tab`          | Switch focus between panels                |

### Session management

| Key   | Action                                                                |
| ----- | --------------------------------------------------------------------- |
| `n`   | Open inline editor to start a new Claude session                     |
| `N`   | Open the New Session tab                                              |
| `a`   | Adopt (attach to) the currently selected session                     |
| `k`   | Send SIGTERM to the selected session (requires UID + comm checks)    |
| `d`   | Run the fleet admiral dry-run reasoner on the selected session       |
| `D`   | Same as `d` — opens reasoner and switches to Detail tab              |
| `e`   | Load the last reasoner proposal into the editor                      |
| `A`   | Apply the current editor buffer to the selected session              |

### Fleet view

| Key   | Action                                                                |
| ----- | --------------------------------------------------------------------- |
| `/`   | Open inline session search (filters by VM name or session name)      |
| `Esc` | Clear active search (Fleet tab) or navigate back to parent tab       |
| `*`   | Cycle status filter (Active / Idle / Dead / All)                     |
| `t`   | Cycle fleet subview (All / Active / Stuck / etc.)                    |
| `l`   | Toggle the ASCII fleet logo at the top of the cockpit                |
| `L`   | Same as `l`                                                           |

### Editor (when editor panel is active)

| Key             | Action                                              |
| --------------- | --------------------------------------------------- |
| `Enter`         | Submit prompt / confirm action                      |
| `Esc`           | Cancel editor and return to session table           |
| `Tab`           | Apply AI-suggested continuation at cursor           |
| `←` / `→`      | Move cursor left / right                            |
| `↑` / `↓`      | Move cursor to previous / next line                 |
| `Home` / `End`  | Jump to start / end of current line                 |
| `Ctrl-U`        | Clear current line                                  |
| `t` / `T`       | Cycle the editor action (send_input → wait → escalate → mark_complete → restart) |

### Project management

| Key | Action                                                  |
| --- | ------------------------------------------------------- |
| `p` | Prompt for a project path to add to the tracked list   |
| `P` | Remove the selected project from the tracked list      |
| `i` | Open the project repo editor (Projects tab)            |

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

### Local dashboard

| Variable                  | Effect                                                |
| ------------------------- | ----------------------------------------------------- |
| `NO_COLOR`                | Disable ANSI colours (same effect as `--no-color`)   |
| `AMPLIHACK_FLEET_FAST_MS` | Override fast-refresh interval (milliseconds)         |
| `AMPLIHACK_FLEET_SLOW_MS` | Override slow-refresh interval (milliseconds)         |

### Azure VM subcommands

| Variable                               | Effect                                                                    |
| -------------------------------------- | ------------------------------------------------------------------------- |
| `AZLIN_PATH`                           | Override the path to the `azlin` binary                                   |
| `AMPLIHACK_FLEET_REASONER_BINARY_PATH` | Override the path to the reasoner binary (default: `claude` on PATH)      |
| `AMPLIHACK_FLEET_EXISTING_VMS`         | Comma- or whitespace-separated list of VMs to exclude from `start` cycles |

#### `AMPLIHACK_FLEET_REASONER_BINARY_PATH` interface contract

The binary pointed to by this variable (or `claude` on `PATH` when the
variable is unset) must satisfy the following contract for the fleet
admiral to use it successfully:

**Invocation**

```sh
<binary> --dangerously-skip-permissions --print < <prompt>
```

The prompt is passed on **stdin**.  The binary is invoked with
`Command::new()` — no shell expansion is performed.

**Exit codes**

| Code | Meaning |
|------|---------|
| `0`  | Reasoning completed; JSON response on stdout |
| non-zero | Reasoning failed; fleet admiral records a dry-run error |

**Stdout format**

The binary must write a single JSON object to stdout.  All other output
must go to stderr (it is captured and written to the log file, not parsed):

```json
{
  "action": "send_input|wait|escalate|mark_complete|restart",
  "input_text": "text to type into the session (only required for send_input)",
  "reasoning": "one-sentence explanation for the choice",
  "confidence": 0.87
}
```

| Field        | Type    | Required       | Constraint                          |
| ------------ | ------- | -------------- | ----------------------------------- |
| `action`     | string  | always         | One of the five action literals     |
| `input_text` | string  | if `send_input`| Sent verbatim; keep under 1 000 c   |
| `reasoning`  | string  | always         | Logged and displayed in the TUI     |
| `confidence` | float   | always         | `[0.0, 1.0]`; below 0.6 downgrades `send_input` to `wait`; below 0.8 downgrades `restart` to `wait` |

A response that is not valid JSON, or that is missing required fields,
causes the fleet admiral to record a dry-run error for that session.

## Python-free execution guarantee

The fleet command and the entire `amplihack` binary are implemented in Rust.
They must **never** invoke a Python interpreter, directly or indirectly, for
any code path that a user or the fleet orchestration loop can reach.

### What the `no_python_probe` tests enforce

The test suite includes a module tagged `no_python_probe` in the `fleet`
crate.  These tests verify, at the Rust unit-test level, that no `fleet`
code path spawns a subprocess named `python` or `python3`.

**What the guarantee covers:**

- The fleet TUI renders without invoking Python.
- `fleet scout`, `fleet advance`, `fleet dry-run`, and all other subcommands
  start without invoking Python.
- The LLM reasoner backend subprocess is always `claude` (or the binary at
  `AMPLIHACK_FLEET_REASONER_BINARY_PATH`) — never a Python script.

**What would cause a `no_python_probe` test to fail:**

- Adding a `Command::new("python")` or `Command::new("python3")` call anywhere
  in `src/commands/fleet.rs` or its dependencies.
- Adding a helper script that the fleet command calls via `Command::new("sh")`
  where the script itself invokes Python.
- Using a Rust crate that spawns a Python subprocess as part of its
  initialization path.

### Running the tests

```sh
# Run only no_python_probe tests:
cargo test -p fleet no_python_probe

# Run the full smoke probe (also validates at runtime with Python stripped
# from PATH):
./scripts/probe-no-python.sh
```

Both must pass on every PR that touches `src/commands/fleet.rs`.

See [Validate No-Python Compliance](../howto/validate-no-python.md) for the
full probe workflow and CI integration instructions.

---

## Session state lifecycle and tempfile behavior

### What survives process exit

| State                       | Persisted? | Location                                            |
| --------------------------- | ---------- | --------------------------------------------------- |
| Tracked project list        | Yes        | `~/.claude/runtime/fleet_dashboard.json`            |
| Last scout results          | Yes        | `~/.claude/runtime/fleet/last_scout.json`           |
| Fleet task queue            | Yes        | `~/.claude/runtime/fleet/queue.json`                |
| Fleet snapshot              | Yes (on demand) | `~/.claude/runtime/fleet/snapshot.json`        |
| In-memory UI state (selection, filters, editor buffer, proposals) | No | Heap only — lost on exit |
| Capture cache (tmux output) | No         | Heap only — lost on exit                            |

### Atomic writes and crash safety

All JSON files written by `amplihack fleet` use a **write-to-temp-then-rename**
pattern:

1. A temporary file is created in the **same directory** as the target file.
2. The JSON content is written to the temp file.
3. `rename(2)` atomically replaces the target.  This is an atomic operation on
   all POSIX filesystems with the source and target on the same mount.

The temp file is created with `tempfile::NamedTempFile` and then persisted with
`persist()`.  Mode `0600` (owner read/write only) is set on the temp file
before the rename so the target inherits correct permissions immediately.

**On crash:** If the binary crashes after step 1 but before step 3, a temporary
file with a random name may be left in `~/.claude/runtime/fleet/` (or
`~/.claude/runtime/`).  These orphaned temp files are safe to delete.  They are
never referenced after a fresh startup.  The previous version of the target file
remains intact because `rename(2)` was not reached.

---

## Security requirements for contributors

Every pull request that modifies `src/commands/fleet.rs` or any file under
`src/commands/fleet/` must satisfy the following checklist before merge.

### Pre-merge checklist

- [ ] **`cargo audit` passes** — Run `cargo audit` and confirm there are no
  CVEs in the dependency tree.  A failing audit blocks merge.  Do not suppress
  findings without a documented justification in the PR description.

- [ ] **`Command::new()` calls are injection-safe** — Every `Command::new()`
  invocation must pass the binary name as a literal string or a path resolved
  at startup (e.g., from `AZLIN_PATH`).  User-supplied strings must **never**
  be passed as the executable name or as unsanitized shell arguments.  Use
  `.arg()` for each argument separately; never concatenate arguments into a
  shell string and pass them via `sh -c`.

  ```rust
  // CORRECT — each argument is a separate .arg() call
  Command::new("azlin")
      .arg("tmux")
      .arg("send-keys")
      .arg(&session_name)   // session_name already sanitized
      .arg(&input_text)     // input_text from reasoner, length-capped

  // INCORRECT — never do this
  Command::new("sh").arg("-c").arg(format!("azlin send {}", user_input))
  ```

- [ ] **`tempfile::persist()` uses mode `0600`** — Any new file written via
  `NamedTempFile::persist()` must set Unix permissions to `0600` before
  persisting.  This prevents other local users from reading fleet state files.

  ```rust
  use std::os::unix::fs::PermissionsExt;
  let mut tmp = tempfile::NamedTempFile::new_in(&parent_dir)?;
  serde_json::to_writer(&mut tmp, &payload)?;
  tmp.as_file().set_permissions(
      std::fs::Permissions::from_mode(0o600)
  )?;
  tmp.persist(&target_path)?;
  ```

- [ ] **CLI input validation** — Any new CLI argument that accepts a session
  name, VM name, or identifier passed to an external process must be validated
  against the allowlist **`[a-zA-Z0-9-]`** (alphanumeric and hyphens only)
  before use.  Reject inputs that fail this check with a clear error message
  before the argument reaches any `Command::new()` call.  This is already
  enforced for session IDs via `sanitize_session_id()`; new identifier
  arguments must use the same function or an equivalent check.

---

## Related

- [How to Use the Fleet Dashboard](../howto/use-fleet-dashboard.md)
- [Run Fleet Scout and Advance on Azure VMs](../howto/run-fleet-scout-and-advance.md)
- [Fleet Dashboard Architecture](../concepts/fleet-dashboard-architecture.md)
- [Fleet Admiral Reasoning Engine](../concepts/fleet-admiral-reasoning.md)
- [Signal Handling and Exit Codes](../reference/signal-handling.md)
- [Environment Variables](../reference/environment-variables.md)
