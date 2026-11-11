---
title: Profile Organisation Affinity
description: Organisation restrictions and durable session warnings for automatic login swaps.
doc_type: reference
review_schedule: as-needed
---

# Profile organisation affinity

Automatic login swaps prefer capacity in the live login's organisation, and
make a confirmed crossing only when capacity forces it. Crossing
organisations can make earlier thinking unavailable on the next
turn. The visible conversation remains available and the session can continue.
Swapping back does not restore earlier thinking.

This policy applies to the rotator, auto-switch and nested rerun selection,
including unread candidates, overflow and fallback paths. It adds restrictions
to the existing [profile rules](profile-command.md); it grants no swap authority.

## Contents

- [Organisation identity](#organisation-identity)
- [Selection rules](#selection-rules)
- [Configuration and operator commands](#configuration-and-operator-commands)
- [Warnings](#warnings)
- [Durable event contract](#durable-event-contract)
- [Session delivery contract](#session-delivery-contract)
- [Storage and recovery](#storage-and-recovery)
- [Compatibility and platform limits](#compatibility-and-platform-limits)

## Organisation identity

The resolver follows the actual hosted login using existing account and hosting
metadata, before the swap changes that metadata. A profile directory's name
does not establish its organisation. Account identity comparison, including
`Account::same()`, does not establish organisation affinity either.

Organisation discovery does not inspect credentials. Existing credential
ownership checks remain separate and authoritative.

Auto-switch reads a login's organisation from its expected record
(`.auto-switch.d/expected/<profile>.json`) through the same reader the swap
guard uses. A record that is missing, unsafe, malformed or without an `org`
entry reads as no record, so the organisation is unknown. The rotator takes
the organisation from its trusted identity reads; its swap guard compares
each read with the same expected record, organisation included, and refuses
the swap on a mismatch. Wherever a record exists, the two producers therefore
agree for every swap that goes ahead.

| Source ID | Destination ID | Affinity |
| --- | --- | --- |
| Present and valid | Present, valid and exactly equal | Same |
| Present and valid | Present, valid and unequal | Crossing |
| Either missing, null, malformed or otherwise unresolved | Any | Unresolved |

Two unknown IDs never establish affinity. IDs must be validated, nonempty
strings; display names and directory names are not substitutes.

Earlier organisation-blind selection ranked eligible accounts by capacity
without using organisation affinity. Account records already stored an `org`
alongside `account`, but that metadata did not restrict ranking. This allowed
threshold, plan-room and upgrade moves across organisations. These findings
follow account metadata and selection behavior, without credential inspection.

## Selection rules

First apply existing eligibility and capacity checks. Then restrict candidates
by affinity, before applying the existing ranking within the permitted set.
Only a confirmed crossing, where both organisations are known and differ, is
restricted. An unknown organisation on either side is not a crossing.

| Final swap purpose | Permitted candidates |
| --- | --- |
| Any, with a known source and an eligible Same target that has capacity | Same targets only |
| Any, otherwise | Same and Unresolved targets |
| Necessary capacity relief, with no eligible Same target that has capacity | Crossing targets as well |
| Upgrade or plan-room | Never a Crossing target |

A better headroom score in another organisation does not override available
same-organisation capacity. Missing metadata never keeps a session where it
is: a session on a paid overflow login whose organisation is unknown still
returns to a plan login once one recovers. Before this policy existed it did,
and an unknown organisation does not change that. Such a swap carries the
Unresolved warning.

Overflow and fallback selection use the final purpose of the move. A fallback
cannot turn a preference-only move into capacity necessity or into a
confirmed crossing. Unread auto-switch candidates receive the same checks;
absence of a reading does not establish capacity.

Authorisation, exclusions, leases, spend limits, pins, freshness, gain and
capacity rules still apply. Organisation affinity cannot make an otherwise
ineligible target eligible. Before completing a swap, execution re-reads the
source and target organisations under the execution locks and refuses the
swap if either changed since selection.

The rotator, auto-switch swap and nested rerun share one policy function
(`organisation_affinity::pools`), called once per decision, which returns both
the selection pool and the plan-room pool. Auto-switch reaches it through one
helper (`auto_switch/affinity.rs`).

### Selection examples

These are policy examples, using profile names from the rotation guide:

| Situation | Result |
| --- | --- |
| `work` needs capacity; `spare` is Same and eligible; `personal` is Crossing with more headroom | Select within Same capacity, preserving its existing ranking |
| `work` needs capacity; no Same target has capacity; `personal` passes all relief checks | Crossing may proceed, with a confirmed-crossing warning |
| `work` requests an upgrade; `personal` is the only better target and is Crossing | No upgrade |
| `work` requests an upgrade; `personal` is the only better target and its ID is unknown | Upgrade, with a risk warning |
| `work` is a paid overflow login with no recorded organisation; plan login `spare` recovers | Back to `spare`, with a risk warning |
| Plan-room fallback finds only Crossing targets | No plan-room swap |

## Configuration and operator commands

The affinity policy is automatic and has no separate opt-out or organisation
override. Existing thresholds, exclusions, pins and overflow settings keep their
meaning but cannot bypass affinity restrictions. No new tuning flag is required.

The existing operator commands remain the entry points:

| Command | Relevant behavior |
| --- | --- |
| `amplihack profile rotate --dry-run` | Previews selection under the affinity policy without moving a login or publishing a completed warning event |
| `amplihack profile rotate` | Applies the policy and records completed swaps |
| `amplihack profile rotate status` | Reports confirmed crossings and unresolved risks for operator inspection |
| `amplihack profile auto-switch status` | Reports auto-switch affinity outcomes without waking sessions |

For setup and command examples, see [Rotate Claude Logins](../howto/claude-profile-rotation.md)
and [Switch Accounts Automatically](../howto/claude-profiles.md#switch-accounts-automatically).

## Warnings

Rotate logs, operator status and session delivery use the same distinction:

| Outcome | Warning text |
| --- | --- |
| Crossing | Organisation crossing: earlier thinking becomes unavailable on the next turn. The visible conversation remains available and the session can continue. |
| Unresolved | Organisation affinity is unresolved: earlier thinking may become unavailable on the next turn. The visible conversation remains available and the session can continue. |
| Same | No cross-organisation thinking-loss warning |

Diagnostics identify the swap event and producer, distinguishing rotator from
auto-switch activity. Metadata displayed in terminals is escaped. Warning
records contain no credentials, tokens or conversation content.

## Durable event contract

Completed swaps record source and destination organisation IDs, an explicit
Same, Crossing or Unresolved outcome, and sufficient event identity and scope
to deliver warnings independently to affected sessions. Event records include:

- A supported schema version and synchronized, monotonically ordered event ID.
- The producer and final swap reason.
- Source and destination organisation IDs, with unknown values represented explicitly.
- Affinity consistent with those IDs.
- The canonical profile root and affected registered session identities.

The completed event history is separate from the latest rotator resume signal.
A single overwritten latest-swap record is not sufficient warning storage.
For example, Crossing event 41 followed by Same event 42 leaves event 41
pending for every affected session that has not consumed it.

This is an internal persistence contract, not a public file-editing API.
Record versions, sizes, collection bounds, ordering, producer, reason, scope
and affinity consistency are validated before use.

## Session delivery contract

Trusted lifecycle registration binds a session identity to its canonical
profile root before a relevant swap can be missed. Registration coordinates
its initial event position with publication. Caller-supplied identifiers alone
cannot register a session, read another session's warnings or advance its cursor.

At the next limit-resume handling that reaches the session, each affected
session receives pending warnings in event order, either in the tmux resume keys
or in the hook's stderr on an exit-2 wake. Claude Code passes hook stderr to the
session only on exit 2; on exit 0 it reaches the operator at most. Every other
outcome (rotator down, auto-switch off, guard failure, no swap to resume onto,
failed tmux keys) still writes the warnings to stderr for the operator and
leaves them pending for a later stop. Immediate
interruption of every running session is not required. Auto-switch scopes its
events to affected sessions; it does not manufacture rotator resume records or
wake unrelated sessions.

Warning delivery does not alter `judge()` or existing resume authorisation.
The tmux route preserves safe argument and key handling and never interpolates
metadata into shell commands.

Each session has an independent, monotonic consumption cursor. Only a
successfully handled prefix can be acknowledged, after successful delivery to
the session. Failed delivery, and output the session does not read, leaves
warnings pending. If delivery succeeds but cursor
persistence fails, delivery may repeat: the guarantee is at least once.

History needed by an unconsumed registered session is retained. A later swap,
another session's acknowledgment or retention maintenance cannot discard it.
Corrupt or unsupported committed records are reported explicitly; consumption
and retention never silently skip past them.

## Storage and recovery

The private journal lives at `<profile-root>/.swap-warning-history/journal.json`.
Its directory has mode 0700 and its files have mode 0600. One kernel file lock
serializes registration, intent persistence, publication, acknowledgment and
retention; callers take the existing swap lock before the journal lock.
Journal writes use an atomic rename and directory synchronization. Reads reject
unsafe files, unsupported events, inconsistent affinity and corrupt state.

Event IDs increase under that lock. Transaction identities correlate intents
with the existing swap journal: rotator identities include its process, recorded
start and credential inodes; auto-switch identities include its process,
recorded start, host and source/destination login names. Start timestamps are
correlation data, not a delivery clock or a recovery timeout. Publication runs
in the verified completion path before the transaction journal is removed, so
the same recovery path can repeat publication safely after a failure.
Both swap journals retain an explicit warning reference; a credential refresh
between selection and exchange cannot disconnect the intent from recovery.

Lifecycle identities combine the hook parent's kernel PID and process birth
identity with the session ID. macOS reads process birth metadata through
`proc_pidinfo`; Linux reads the process's start field from `/proc`. A reused PID
does not inherit a previous process's cursor. The canonical configuration
directory distinguishes home sessions from sessions in a named profile.

The journal is bounded to 8 MiB. Reaching that bound refuses the new warning
intent rather than discarding warnings already recorded; the swap itself goes
ahead (see [Warnings are advisory](#warnings-are-advisory)). Required history and
ambiguous intents are retained; long-lived unconsumed sessions can therefore
require operator attention. Completion identities are retained while authoritative recovery journals can reference them for idempotence.

State uses owner-private directories (0700) and files (0600), secure temporary
creation, atomic publication and filesystem synchronization. Unsafe ownership,
symlink traversal, unexpected file types and unsafe parent directories are
rejected. One consistent lock order coordinates execution, registration,
event allocation, publication, cursor updates and retention.

A private durable intent is written before login mutation. Pending intents
are not deliverable events. Recovery publishes them only when authoritative transaction evidence
proves completion; ambiguous recovery retains evidence and reports the
inconsistency.

### Warnings are advisory

The swap is the job; the warning tells live sessions about it. No failure of
the warning journal stops or undoes a swap:

- If the intent cannot be written before the move (journal full, unsafe
  directory, unreadable transaction journal), the producer reports it on
  stderr and swaps without a session warning. The swap record and the rotate
  log still record the organisation outcome.
- If publication fails after the move, the producer reports it and finishes
  the swap normally; the transaction journal is removed as for any completed
  swap.
- If an aborted swap's intent cannot be dropped, the producer reports it.
  The intent is never delivered, since only completed transactions publish
  events.

## Compatibility and platform limits

Legacy swap records remain readable. Absent organisation fields mean unknown
historical information. A legacy record alone produces neither a new confirmed
crossing nor a thinking-loss warning. New records explicitly classify affinity;
they do not infer a crossing from absent legacy fields.

Affinity and durable delivery do not remove existing platform restrictions on
rotation or session discovery. See the rotation guide's [limits](../howto/claude-profile-rotation.md#limits).
Linux-specific process discovery and resume mechanisms retain their Linux
requirements. Durability depends on supported filesystem flushing and directory
synchronization. Earlier thinking restoration and measurement of subsequent
work quality are outside this feature's contract.

## Warning failure isolation and retention

Scoped warnings are delivered on limit handling even when the rotator is down.
Warning registration, reading, delivery, and acknowledgement failures do not
change resume authorization. A committed exit-2 wake remains exit 2 when stderr
fails. Sent tmux keys, or stderr written on an exit-2 wake, acknowledge only the
captured warning prefix; failed delivery and exit-0 stderr leave warnings
pending. Persisted-record parse errors
use fixed diagnostics without record contents.

Every journal write compacts consumed and non-warning events before enforcing
the 8 MiB bound. Outstanding warnings and unresolved intents remain durable.
Before a producer prepares its next intent, it retires completed identities
and pending intents that neither authoritative swap journal references. It does
this under the swap lock that the rotator and auto-switch share, so no live swap
can be between writing its intent and writing its journal. Unreadable recovery
journals prevent retirement. This reclaims evidence left by a crash after
recovery journal deletion, and intents left by a crash before the journal was
written, without an age cutoff or a background service.

### Lifecycle retention

On journal writes, registrations are retired only when kernel process metadata
proves the process exited or its PID has been reused. Permission errors and
other unresolved process lookups retain the registration. Pending transaction
scopes remain intact until the transaction is resolved. Consumed events and
warnings whose registered processes have demonstrably ended are compacted.
No timer or age cutoff authorizes deletion.

### Selection rationale

Strict capacity relief and plan-room fallback use separate candidate pools.
The existing selectors define capacity using the configured thresholds; affinity
does not introduce a second percentage limit. Same-organisation room above a
threshold cannot hide a different organisation's eligible capacity relief.
When no strict relief target is selected, existing plan-room policy can still
choose an eligible, confirmed same-organisation target with room.

### Validation and platform limits

Deterministic regressions use temporary fake profiles and injected events for
organisation selection, threshold boundaries, unknown metadata, legacy records,
intervening swaps, delivery scope and kernel lifecycle retention. They do not
need a live Claude session or real profile roots.

Linux process discovery, FIFO and live resume integration remain Linux-specific
and require CI validation. macOS can validate the shared affinity policy,
warning history, injected delivery routes and CLI library tests. Full workspace
validation can encounter existing macOS-only integration compilation failures;
such failures are not passing evidence for the affected checks. Hook validation
must run without bypasses for the final revision before release.

An abrupt process exit between persisting a warning intent and linking it from
the producer's transaction journal leaves an orphan intent. Ordinary verified
failure and rollback paths abort their scoped intent. An orphan intent is never
deliverable, because only recovery through a journal publishes an intent, so the
next producer's preparation clears it under the shared swap lock, together with
its hold on the sessions it scoped.
