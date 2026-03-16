# How to Use the Fleet Dashboard

The fleet dashboard gives you a single terminal view of every running Claude
session on the machine.  From one screen you can start a new session, adopt
(attach to) an existing one, and manage the projects you want to track — all
without leaving your terminal.

## Contents

- [Open the dashboard](#open-the-dashboard)
- [Read the session table](#read-the-session-table)
- [Search sessions](#search-sessions)
- [Start a new session](#start-a-new-session)
- [Adopt an existing session](#adopt-an-existing-session)
- [Inspect a session in the Detail tab](#inspect-a-session-in-the-detail-tab)
- [Run the fleet admiral reasoner](#run-the-fleet-admiral-reasoner)
- [Understand dry-run failure notices](#understand-dry-run-failure-notices)
- [Apply a reasoner proposal](#apply-a-reasoner-proposal)
- [Understand persistent apply failure notices](#understand-persistent-apply-failure-notices)
- [Cycle editor action choices](#cycle-editor-action-choices)
- [Add and remove tracked projects](#add-and-remove-tracked-projects)
- [Use the inline editor](#use-the-inline-editor)
- [Toggle the fleet logo](#toggle-the-fleet-logo)
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

## Search sessions

Press `/` while the Fleet tab is active to open an inline search bar at the
bottom of the cockpit.

```
Search: amplihack-rs (press / to edit, Esc to clear)
```

Type any substring of a VM name or session name.  The session table filters in
real time as you type.  Only rows whose VM name or session name contains the
search string are shown.

| Key   | Action                                           |
| ----- | ------------------------------------------------ |
| `/`   | Open search input (starts a new search term)     |
| `Esc` | Clear search and restore the full session table  |

The search string is case-sensitive.  To reset a search without pressing `Esc`,
press `/` again and clear the input.

When both a status filter (`*`) and a search are active simultaneously, a
session must satisfy **both** conditions to appear:

```
No sessions match the current filter/search. Press Esc or '*' to clear.
```

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

## Inspect a session in the Detail tab

1. Use `↑` / `↓` to select a session row in the Fleet tab.
2. Press `Enter` or navigate to the **Detail** tab with `Tab`.

The Detail tab shows:
- A live preview of the session's terminal output (refreshes every 5 s via
  `tmux capture-pane`).
- The most recent reasoner proposal for the session, if one has been generated.
- Control hints at the bottom of the panel:
  ```
  e reload  i focus editor  t cycle action  A apply edited
  ```

The detail view refreshes automatically in the background.  Press `Tab` to
return to the Fleet tab.

---

## Run the fleet admiral reasoner

From the Fleet tab, with a session selected:

1. Press `d` (or `D`).  The reasoner analyses the selected session using the
   configured backend (Claude by default).
2. The cockpit switches to the Detail tab automatically.
3. A proposal notice appears above the terminal capture:
   ```
   Proposed action: send_input (87%)
   Input: "Please run the tests and commit the result."
   Reason: Session appears idle — tests have not been run since last commit.
   ```

> **Reasoner timeout** — The reasoner runs with a 180 s timeout.  If the
> backend does not respond within that window, the reasoner returns an error,
> the proposal field is replaced with a dry-run failure notice (not left blank),
> and the cockpit returns to its normal responsive state — it is not stuck
> waiting.  Press `d` to retry once the backend is available.

---

## Understand dry-run failure notices

When the fleet admiral reasoner cannot produce a proposal — because the
backend timed out, returned malformed JSON, or encountered a subprocess error
— the Detail tab shows a **dry-run failure notice** in place of the proposal:

```
Reasoner error: backend subprocess exited with code 1
Press 'd' to retry.
```

The failure notice:
- Is **persistent** across refresh cycles.  It does not disappear on the next
  5 s refresh; it stays until you retry (`d`) or navigate away.
- Contains only the error category, not internal paths or PIDs.  Full detail is
  in `~/.claude/runtime/logs/`.
- Replaces any previous successful proposal for the same session.

**Recovery steps**

| Symptom                        | Action                                              |
| ------------------------------ | --------------------------------------------------- |
| "backend subprocess exited…"   | Check `AMPLIHACK_FLEET_REASONER_BINARY_PATH`; ensure `claude` is on PATH |
| "reasoner timed out"           | The session may have large output; reduce `--capture-lines` |
| Notice appears immediately     | Verify `azlin` is installed and reachable            |

---

## Apply a reasoner proposal

After a successful dry run the Detail tab shows the proposal and control hints:

```
e reload  i focus editor  t cycle action  A apply edited
```

| Key   | Action name          | What it does                                                                     |
| ----- | -------------------- | -------------------------------------------------------------------------------- |
| `a`   | **Apply direct**     | Send the reasoner's last prepared proposal to the session immediately, without opening the editor |
| `A`   | **Apply edited**     | Send the current editor buffer (use after editing with `i`) to the session       |
| `e`   | **Reload to editor** | Copy the last reasoner proposal back into the inline editor buffer so you can modify it before applying |
| `i`   | **Focus editor**     | Activate the inline editor input so you can type or amend the input text         |
| `t`   | **Cycle action**     | Step through available action types for the current proposal (see [Cycle editor action choices](#cycle-editor-action-choices)) |

Pressing `a` (**Apply direct**) sends the reasoner's proposed action (e.g.,
`send_input`) to the session immediately, without opening the editor.

Pressing `e` (**Reload to editor**) followed by `i` (**Focus editor**) lets you
edit the proposed text, then press `A` (**Apply edited**) to send the modified
version.

---

## Understand persistent apply failure notices

When an apply attempt fails — because `azlin` is unreachable, the tmux target
session has exited, or the input could not be sent — the Detail tab shows a
**persistent apply failure notice**:

```
Apply failed: tmux send-keys returned exit code 1
Last action: send_input -> amplihack-vm-01/work-session-3
```

Key properties:
- The notice is **session-scoped**.  It stays anchored to the specific
  `vm/session` pair that failed; navigating to a different session shows that
  session's state, not the error.
- The notice is **cleared on the next successful apply** to the same session.
  A partial retry that also fails replaces the old notice with the new error.
- Full diagnostic detail (exit code, stderr, absolute path) goes to
  `~/.claude/runtime/logs/`; the TUI shows only the error category.

---

## Cycle editor action choices

Press `t` (or `T`) when a proposal is loaded in the editor to cycle through the
available actions for that proposal:

```
send_input → wait → escalate → mark_complete → restart → send_input → …
```

The status bar confirms the change:

```
Editor action set to wait for amplihack-vm-01/work-session-3.
```

Use this when you want to change the action recommended by the reasoner without
rerunning the full dry-run.  For example, if the reasoner proposes `send_input`
but you want to `mark_complete` the session instead:

1. Press `d` to run the reasoner.
2. Press `e` to load the proposal into the editor.
3. Press `t` repeatedly until the status bar shows `mark_complete`.
4. Press `A` to apply.

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

## Toggle the fleet logo

Press `l` (or `L`) at any time to show or hide the ASCII fleet logo at the top
of the cockpit.  The logo is hidden by default to maximise the visible session
table area.  Toggle it on for a more visual experience on wide terminals.

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
