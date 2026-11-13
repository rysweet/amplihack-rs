---
title: Follow an event-driven rotation
description: Start a watcher and follow usage and login changes without interval polling.
doc_type: tutorial
review_schedule: as-needed
---

# Follow an event-driven rotation

This exercise shows how session observations drive the background rotator.
Use Linux for credential movement; macOS supports native watching but retains
the [rotation restrictions](../howto/claude-profile-rotation.md#limits).

You need amplihack and Claude Code installed, a default Claude login and at
least one other logged-in profile. Start with no rotator running and no manual
rotation hold. See [profile setup](../howto/claude-profiles.md) for these
prerequisites. The commands use your configured profile root and can move
logins when a threshold is reached.

## 1. Preview the current choice

```sh
amplihack profile rotate --dry-run --session-at 85 --week-at 90
```

The preview prints a `cycle` line and a proposed outcome, such as `would
stay` or `would swap`. It moves no login. Resolve any reported identity,
eligibility or organisation problem before continuing.

## 2. Start the watcher

```sh
amplihack profile auto-switch on
amplihack profile rotate start --session-at 85 --week-at 90
amplihack profile rotate status
```

`start` prints `rotate: started`, a PID, and the log and root paths. It
returns after launching the child; baseline loading and native watch setup
continue asynchronously. `status` reports process liveness and is not a
readiness acknowledgement. Child setup failures appear in `rotate.err`
beside the log.

The watcher registers containing directories, reads stored observations and
reconciles events queued during setup. Existing logins form its baseline;
startup does not probe every login. Leave `--interval` unset: the watcher
reacts to relevant changes without a schedule.

## 3. Produce session observations

Launch an ordinary session:

```sh
amplihack claude
```

The full session-start hook runs. As the session produces usage, its status
line publishes usage observations to its captured profile and login
generation. Publications remain attributed to that profile across swaps.
A recognised limit event can also supply evidence.

An ordinary observation requires both windows, or a weekly window and an
explicitly absent session limit. Missing data cannot establish capacity. See
the [window contract](../reference/usage-observations.md#window-representation-and-validation).

A meaningful update crossing 85% session use or 90% weekly use triggers one
logical decision. Duplicate filesystem notifications do not repeat it. The
watcher requests missing evidence only for logins that decision needs, then
checks eligibility, organisation affinity and safety before moving a login.
A changed reset timestamp also counts as new input when percentages stay
equal. It triggers one decision; advancing time alone does not. See the
[reset-aware contract](../reference/usage-observations.md#reset-aware-decisions).

After such an event, inspect the result:

```sh
amplihack profile rotate status
```

Read the log at the path printed by `start` for the decision and any refusal.
A swap requires enough headroom and all safeguards to pass. To request an
explicit decision without waiting for session activity, run:

```sh
amplihack profile rotate
```

## 4. Recognise login changes

A login added during startup discovery may become part of the baseline.
After setup, a new login receives exactly one logical probe when a pending
decision needs its usage and that generation has no valid observation.
Unchanged logins are not probed merely because another login was added.

Re-login creates a distinct generation. Removing a login retires its
generation. Earlier observations and late publications captured for that
generation cannot become evidence for its replacement. A failed probe
returns the watcher to blocking; elapsed time does not retry it.

## 5. Stop the watcher

```sh
amplihack profile rotate stop
```

The command verifies and stops the watcher, then prints `rotate: stopped`
with its PID. For continued use, follow the
[background rotation guide](../howto/claude-profile-rotation.md#run-rotation-in-the-background).
The [event model](../concepts/event-driven-rotation.md) explains completion,
recovery and concurrent changes.
