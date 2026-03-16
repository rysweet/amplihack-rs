# How to Use the Fleet Dashboard

The fleet dashboard gives you a single terminal view of every running Claude
session on the machine.  From one screen you can start a new session, adopt
(attach to) an existing one, and manage the projects you want to track — all
without leaving your terminal.

## Contents

- [Open the dashboard](#open-the-dashboard)
- [Read the session table](#read-the-session-table)
- [Start a new session](#start-a-new-session)
- [Adopt an existing session](#adopt-an-existing-session)
- [Add and remove tracked projects](#add-and-remove-tracked-projects)
- [Use the inline editor](#use-the-inline-editor)
- [Exit cleanly](#exit-cleanly)
- [Troubleshoot common problems](#troubleshoot-common-problems)

---

## Open the dashboard

```sh
amplihack fleet
```

The terminal switches to raw mode and the cockpit renders immediately.  Press
`q` at any time to return to your shell.

> **Non-interactive environments** — CI pipelines and scripts should not call
> `amplihack fleet` directly.  The command requires a real TTY.  Use
> `amplihack fleet --help` (exit 0) for existence checks.

---

## Read the session table

The top panel shows one row per session discovered in
`~/.claude/runtime/locks/`.

```
ID              STATUS   PID     PROJECT                   AGE
────────────────────────────────────────────────────────────────
abc123-def456   Active   12345   ~/src/amplihack-rs        2m 14s
789xyz-000111   Idle     9871    ~/src/my-project          18m 03s
dead-session    Dead     —       ~/src/old-project         —
```

| Column    | Meaning                                                      |
| --------- | ------------------------------------------------------------ |
| `ID`      | Sanitized session identifier (alphanumeric, hyphens, underscores only) |
| `STATUS`  | `Active` (green), `Idle` (yellow), `Dead` (red), `Unknown` (grey) |
| `PID`     | OS process ID; `—` when the process no longer exists         |
| `PROJECT` | Working directory recorded in the lock file                  |
| `AGE`     | Time since the lock file was last modified                   |

The session list refreshes every **500 ms**.  The content preview panel (right
side, when a session is selected) refreshes every **5 s** via `tmux
capture-pane`; it is skipped gracefully when `tmux` is absent.

---

## Start a new session

1. Press `n` in the dashboard.
2. The inline editor opens at the bottom of the screen.
3. Type your opening prompt (up to **1 000 characters**; the counter shows
   `42/1000`).
4. Press `Enter` to launch.  The session appears in the table within the next
   refresh cycle.

The new session spawns in the background — the dashboard remains responsive
while it starts.

**Editor limits** *(security limits — not soft suggestions)*

The dashboard enforces hard limits to prevent oversized prompts from
exhausting memory or being passed to subprocesses unchecked.  A live counter
in the status bar (e.g., `147/200 lines`) shows current usage against the
line cap.

| Limit           | Value   | Behaviour when exceeded           |
| --------------- | ------- | --------------------------------- |
| Characters/line | 4 096   | Further input is silently dropped |
| Lines           | 200     | Further lines are silently dropped |
| Prompt cap      | 1 000 c | Prompt is truncated before handoff |

---

## Adopt an existing session

1. Use `↑` / `↓` to select a session row.
2. Press `a` to adopt it.

Adoption validates three conditions before doing anything:

1. **PID range** — must be between 1 and 4 194 304 (Linux kernel maximum).
2. **Owner UID** — the process owner must match your UID.  The dashboard
   verifies this before any signal is sent.  You **cannot** adopt sessions
   owned by other users, including `root`.  Adoption fails with a permission
   error when there is a UID mismatch — this is intentional security
   behaviour, not a bug.
3. **Process name** — `/proc/{pid}/comm` (Linux) or the macOS `sysctl`
   equivalent must match the expected Claude process name.

If any check fails the dashboard shows a status-bar message and takes no
action.  No signal is ever sent to a mismatched PID.

---

## Add and remove tracked projects

The fleet dashboard persists a list of projects you care about in
`~/.claude/runtime/fleet_dashboard.json` (created with mode `0600` on first
save).

| Key    | Action                                                         |
| ------ | -------------------------------------------------------------- |
| `p`    | Open project-path prompt; type an absolute path and press Enter |
| `P`    | Remove the selected project from the tracked list              |

`add_project` calls `canonicalize()` before storing the path and rejects
anything that is not an existing directory.  `remove_project` uses `retain()`
so order is preserved for all remaining entries.

---

## Use the inline editor

The editor is the same component used for new-session prompts and is also
accessible when an AI proposal arrives (press `Enter` to accept it into the
buffer at the current cursor position).

**Cursor movement**

| Key            | Movement                       |
| -------------- | ------------------------------ |
| `←` / `→`     | Character left / right         |
| `↑` / `↓`     | Line up / down                 |
| `Home` / `End` | Start / end of current line    |

**Applying a proposal**

When the AI suggests continuation text a `[TAB to apply]` hint appears.
Press `Tab` to insert the proposal at the cursor.  The text is validated as
valid UTF-8 before insertion; a failed validation silently discards the
proposal and logs the event to `~/.claude/runtime/logs/`.

---

## Exit cleanly

| Key       | Action                                       |
| --------- | -------------------------------------------- |
| `q`       | Exit the dashboard and restore the terminal  |
| `Ctrl-C`  | Same as `q` — RAII guard restores raw mode   |

The terminal is guaranteed to be restored even if the dashboard panics: a
`TerminalGuard` struct wraps `crossterm::terminal::disable_raw_mode()` and
runs its `Drop` impl unconditionally.

---

## Troubleshoot common problems

### Terminal is garbled after an unexpected crash

```sh
reset
```

This is the nuclear option.  In practice the `TerminalGuard` drop handler
prevents this, but `reset` always works.

### Sessions appear as `Dead` immediately

The lock file exists but the PID is gone.  This is normal for sessions that
ended without cleaning up their lock files.  Dead entries are shown for
visibility; they do not affect healthy sessions.

### Content preview never updates

`tmux` is not installed or the session is not running inside a tmux pane.  The
fast-refresh session table still works.  Install `tmux` and run your sessions
inside a tmux pane to enable content previews.

### `add_project` silently rejects my path

The path either does not exist yet, is a file (not a directory), or contains
a symlink that `canonicalize()` cannot resolve.  Create the directory first:

```sh
mkdir -p ~/src/my-new-project
# then press 'p' in the dashboard and enter the path
```

### Fleet dashboard is missing from `--help`

You are running a version older than v0.5.0.  Check:

```sh
amplihack --version
# amplihack 0.5.0
```

Rebuild or reinstall to get v0.5.0.

> **Note on version numbering** — The `v0.4.x` range was intentionally
> skipped.  Those version tags were used on the parallel `amploxy` branch
> during the same development period; skipping to v0.5.0 avoids a tag
> collision.  There was never a released `v0.4.x` of `amplihack-rs`.  See
> `CHANGELOG.md` for the full history.

### Adoption silently fails in a container environment

Linux container namespaces can allocate PIDs higher than 4 194 304.  The
dashboard rejects PIDs outside the range `1..=4_194_304` as a security
measure.  This is intentional behaviour, not a bug.  If you are running
Claude inside a container, start a new session from within the same
namespace rather than trying to adopt from outside it.

### Adoption fails with a permission error

The session you selected is owned by a different OS user (e.g., `root`).
The dashboard verifies process ownership before sending any signal.  You
can only adopt sessions that your UID owns.

---

**See also**

- [amplihack fleet — CLI reference](../reference/fleet-command.md)
- [Fleet Dashboard Architecture](../concepts/fleet-dashboard-architecture.md)
- [Signal Handling and Exit Codes](../reference/signal-handling.md)
