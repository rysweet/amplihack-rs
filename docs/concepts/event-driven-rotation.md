---
title: Event-driven Claude login rotation
description: How filesystem changes and probe completions drive rotation without scheduled work.
doc_type: explanation
review_schedule: as-needed
---

# Event-driven Claude login rotation

The rotator reacts to usage observations, login lifecycle changes and
relevant control changes. With unchanged inputs, it performs no recurring
work after startup. Advancing the clock cannot cause a probe, retry,
decision, rotation or maintenance pass.

For commands, see the [rotator guide](../howto/claude-profile-rotation.md).
For record validation and publication, see the
[usage observation contract](../reference/usage-observations.md).

## Contents

- [Startup establishes a baseline](#startup-establishes-a-baseline)
- [One change owns one decision](#one-change-owns-one-decision)
- [Failure returns to blocking](#failure-returns-to-blocking)
- [Safety checks stay at the transaction boundary](#safety-checks-stay-at-the-transaction-boundary)
- [Time is a safeguard input, not a wake source](#time-is-a-safeguard-input-not-a-wake-source)
- [Overflow recovery does one reconciliation pass](#overflow-recovery-does-one-reconciliation-pass)
- [Why probes skip session initialisation](#why-probes-skip-session-initialisation)

## Startup establishes a baseline

The watcher registers root and parent directories before loading the
baseline, then covers discovered objects and reconciles events queued during
setup. This closes the gap between reading state and establishing watches,
including atomic file replacement, login addition, removal, re-login and
directory replacement. It reads each existing observation once, completes
reconciliation, then signals readiness and blocks.

Production launch is asynchronous. `start` reports that it has spawned the
watcher and recorded its PID; it does not wait for baseline loading or
receive a readiness acknowledgement. The watcher's readiness event marks
completed baseline loading and watch establishment for event-driven tests.
A readiness log, PID file or `status` showing `running` is not a parent
acknowledgement. Setup failures appear in the child's diagnostics.

Profiles discovered during startup form the baseline. Starting or restarting
the watcher does not probe every login, including logins without an
observation. When a pending decision needs a newly added login's usage and
that generation has no valid observation, it receives exactly one logical
probe. Adding a login does not authorise probes of unchanged logins.

Linux uses inotify and macOS uses kqueue. Both block on filesystem,
completion and shutdown readiness without deadlines or periodic rescans.
Descriptors are close-on-exec. Native event support does not remove the
existing platform restrictions on credential movement, process verification
or automatic resume.

## One change owns one decision

Filesystem notifications are reconciliation hints, not permission to move
credentials. The watcher compares meaningful identity, generation, usage,
eligibility and control inputs, including exclusions, organisation
restrictions and pins. Usage percentages, limit flags and reset
timestamps all matter: a changed reset is meaningful even when percentages
stay equal. The [reset-aware contract](../reference/usage-observations.md#reset-aware-decisions)
preserves and compares both resets. Duplicate notifications, reporting metadata,
capture-ticket allocation and expected writes from the rotator do not start
new policy decisions.

A meaningful usage update crossing the configured threshold starts exactly
one logical decision. If that decision needs evidence missing from a login's
current generation, it requests that evidence. Otherwise it consumes the
accepted observation without probing. Observation age alone never requests
a measurement.

Ordinary observations require a complete plan reading: a weekly window and
either a measured session window or explicit confirmation of no session limit.
The [window contract](../reference/usage-observations.md#window-representation-and-validation)
shares publication, read and ranking validation. Recognised limit evidence
can request relief without inventing missing percentages or capacity.

Probe jobs belong to the requesting decision and captured generation. Their
completion continues that decision; it does not start another one. An
authoritative completion queue retains results even when wake signals
coalesce. Concurrent external changes remain pending, are reconciled and
are processed without losing meaningful inputs. A relevant change can
invalidate a pending swap even when it arrives during measurement.

The coordinator bounds pending work and concurrent jobs. It does not start
a second job for the same generation-bound request while one is running.
Stale or superseded completions are discarded rather than republished as
new measurements.

## Failure returns to blocking

A failed probe leaves usage unknown and blocks again. Only a relevant
external dependency change can permit another attempt, and the resulting
decision must still require the reading. Unrelated file activity, duplicate
notifications and time passing do not retry a failed request.

Subprocess timeouts and isolation still protect a probe already in flight.
Their completion reports the outcome to its owner; they never become
sources of new watcher decisions or retries. No retry gap or decision
deadline replaces the former interval loop.

## Safety checks stay at the transaction boundary

Rotation identifies the live login and eligible candidates under the state
lock, measures without the lock, then revalidates and performs the journalled
swap under the lock. Revalidation checks identity, generation, eligibility,
organisation affinity and relevant policy. A changed input cannot authorise
movement based on an earlier snapshot. Missing organisation information
refuses new automatic movement, including overflow recovery and fallback
selection. It cannot grant an exception to affinity checks. The
[affinity reference](../reference/profile-organisation-affinity.md#selection-rules)
defines permitted moves; unresolved historical records remain readable.

Live-reading and swap-back safeguards remain in force. A target's old reading
cannot establish recovery from an observed limit, and rereading or publishing
the same evidence cannot manufacture confirmation. Where further confirmation
is required, rotation waits for distinct qualifying evidence. Unknown usage
does not establish exhaustion, capacity or consent to paid overflow.

## Time is a safeguard input, not a wake source

Hold periods, reset boundaries, expiry and cooldowns are evaluated when a
relevant event arrives or an explicit command runs. Passing a boundary alone
neither wakes the watcher nor proves that a login has capacity again.

The watcher decides on accepted measurements, without the former
interval-based projection or frozen-reading counters. Explicit `rotate now`
commands retain their existing projection rules. Unchanged percentages,
limit flags and reset timestamps with a new revision do not manufacture
recovery evidence. Hand-change confirmation requires distinct qualifying
evidence rather than a later cycle or an elapsed confirmation gap.

Startup performs necessary maintenance once. Later hook repair, recovery
and archive maintenance run on relevant events or through existing explicit
commands. The legacy `sessions.daily_archive` setting enables archive
maintenance but promises no daily execution. An archive date cannot wake
an idle watcher.

## Overflow recovery does one reconciliation pass

After lost filesystem events, including Linux `IN_Q_OVERFLOW`, the watcher
re-establishes registrations and reads every observation file once. It
reconciles changes arriving during recovery and returns to blocking.
Recovery does not perform a speculative probe sweep. In-flight and terminal
probe request identities survive reconciliation, so recovery cannot retry a
failed request or duplicate an outstanding one. Probes remain subject to the
same decision-required evidence rules. Directory replacement or invalidated
subscriptions also require watch repair; subsequent publications remain
observable. An unrecoverable watcher error terminates visibly rather than
spinning on the failed subscription.

The deprecated `--interval` option and `AMPLIHACK_ROTATE_INTERVAL` variable
remain accepted and validated for compatibility. Their values affect no
watcher scheduling, retries, projection cadence or measurement scheduling.

## Why probes skip session initialisation

A Claude usage probe starts a session. Without invocation-specific
suppression, that session enters amplihack's full session-start path,
including context and preference loading, original-request capture,
code-graph setup and compatibility checks. Probe commands set the exact
marker `AMPLIHACK_USAGE_PROBE=1` before sandbox wrapping; applicable hooks
recognise it before expensive initialisation. Primary and fallback commands
share that preparation.

Ordinary interactive and worker commands clear inherited probe suppression
and retain full session initialisation. No user-wide hook configuration
changes. Probes retain `--no-session-persistence` and the existing Linux
projects-directory tmpfs protection.

The presence of recursive scanning or code-graph work on an ordinary
session-start path does not establish its CPU cost. Attributing that cost
requires profiling stages in isolated fixtures; suppressing probe work
alone provides no evidence of a per-function speed-up in real sessions.

## Session-start investigation

Session-start has opt-in stage measurements through
`RUST_LOG=amplihack_hooks::session_start=debug`. Each completed stage reports
elapsed microseconds. These measurements schedule no work and are not
acceptance thresholds.

An isolated macOS debug build was run once per fixture on 12 November 2026.
Each fixture had its own home, framework root, profile root and project.
Automatic indexing was disabled, and no existing graph was supplied.
Child CPU was measured with `getrusage(RUSAGE_CHILDREN)` around each completed
invocation; stage figures are elapsed time, not CPU attribution.

| Fixture | Child user + system CPU | Code-graph setup elapsed |
|---------|-------------------------|--------------------------|
| Ordinary session, no source files | 0.01085 s | 161 microseconds |
| Ordinary session, 2,000 source files | 0.02045 s | 14,716 microseconds |
| Marked probe, 2,000 source files | 0.00840 s | Stage did not run |

Established findings: ordinary setup calls `check_index_status`, which
recursively visits source directories even with automatic indexing disabled.
An ordinary session also reaches graph compatibility and context loading;
context loading calls `summarize_code_graph`. The probe marker returns before
these stages. Interactive and recipe-worker tests exercise the ordinary path.

The fixtures do not reproduce the ten-second CPU cost reported on candide.
A large directory tree, followed directory links, and opening an existing
graph remain hypotheses for that host. The graph-opening cost was not
measured here because these fixtures contain no graph. No scanning or graph
optimisation is claimed from this evidence; the change removes unnecessary
probe sessions and their hook work.
