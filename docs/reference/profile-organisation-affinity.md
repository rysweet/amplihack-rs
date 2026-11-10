---
title: Profile Organisation Affinity
description: Organisation restrictions and durable session warnings for automatic login swaps.
doc_type: reference
review_schedule: as-needed
---

# Profile organisation affinity

Automatic login swaps prefer capacity in the live login's organisation.
Crossing organisations can make earlier thinking unavailable on the next
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
ownership checks remain separate and authoritative. Operational metadata read
errors remain errors; they do not relax selection restrictions.

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

| Final swap purpose | Permitted candidates |
| --- | --- |
| Necessary capacity relief, with an eligible Same target that has capacity | Same targets with capacity only |
| Necessary capacity relief, with no eligible Same target that has capacity | Crossing or Unresolved targets may be considered under existing policy |
| Upgrade | Same only |
| Plan-room | Same only |
| Preference alone | Cannot justify Crossing or Unresolved |

A better headroom score in another organisation does not override available
same-organisation capacity. Unknown source identity blocks upgrades and
plan-room moves, even if the destination has a known ID.

Overflow and fallback selection use the final purpose of the move. A fallback
cannot turn a preference-only move into capacity necessity or bypass confirmed
affinity requirements. Unread auto-switch candidates receive the same checks;
absence of a reading does not establish capacity or affinity.

Authorisation, exclusions, leases, spend limits, pins, freshness, gain and
capacity rules still apply. Organisation affinity cannot make an otherwise
ineligible target eligible. Before completing a swap, execution revalidates
identities, organisation affinity and existing target checks under the execution
locks, including whether same-organisation capacity has become available.

### Selection examples

These are policy examples, using profile names from the rotation guide:

| Situation | Result |
| --- | --- |
| `work` needs capacity; `spare` is Same and eligible; `personal` is Crossing with more headroom | Select within Same capacity, preserving its existing ranking |
| `work` needs capacity; no Same target has capacity; `personal` passes all relief checks | Crossing may proceed, with a confirmed-crossing warning |
| `work` requests an upgrade; `personal` is the only better target and its ID is unknown | No upgrade |
| Plan-room fallback finds only Crossing targets | No plan-room swap |
| Both source and target organisations are unknown during necessary relief | Unresolved may proceed only under the relief rules, with a risk warning |

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

At the next applicable limit-resume handling, each affected session receives
pending warnings in event order through stderr or tmux resume keys. Immediate
interruption of every running session is not required. Auto-switch scopes its
events to affected sessions; it does not manufacture rotator resume records or
wake unrelated sessions.

Warning delivery does not alter `judge()` or existing resume authorisation.
The tmux route preserves safe argument and key handling and never interpolates
metadata into shell commands.

Each session has an independent, monotonic consumption cursor. Only a
successfully handled prefix can be acknowledged, after successful delivery.
Failed delivery leaves warnings pending. If delivery succeeds but cursor
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

The journal is bounded to 8 MiB. Reaching that bound fails explicitly before
an unrecordable swap rather than discarding warnings. Required history and
ambiguous intents are retained; long-lived unconsumed sessions can therefore
require operator attention. Completion identities are retained while authoritative recovery journals can reference them for idempotence.

State uses owner-private directories (0700) and files (0600), secure temporary
creation, atomic publication and filesystem synchronization. Unsafe ownership,
symlink traversal, unexpected file types and unsafe parent directories are
rejected. One consistent lock order coordinates execution, registration,
event allocation, publication, cursor updates and retention.

A private durable intent is written before login mutation. Failure to persist
the intent aborts the swap before mutation. Pending intents are not deliverable
events. Recovery publishes them only when authoritative transaction evidence
proves completion; ambiguous recovery retains evidence and reports the
inconsistency.

If a swap completes but event publication fails, the operation reports that
completion has publication pending and retains recovery evidence. It does not
claim the login mutation was undone. Recovery finishes publication without
losing the affected session scope.

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
fails. Successful stderr or tmux delivery acknowledges only the captured warning
prefix; failed delivery leaves warnings pending. Persisted-record parse errors
use fixed diagnostics without record contents.

Every journal write compacts consumed and non-warning events before enforcing
the 8 MiB bound. Outstanding warnings and unresolved intents remain durable.
Before a producer prepares its next intent, it retires completed identities
that neither authoritative swap journal references. Unreadable recovery journals
prevent retirement. This also reclaims evidence left by a crash after recovery
journal deletion, without an age cutoff or a background service.

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
the producer's transaction journal can leave an orphan intent. Ordinary verified
failure and rollback paths abort their scoped intent. Recovery conservatively
retains an unlinked intent because missing references alone cannot prove the
login was unchanged. Such retained intents count toward the storage bound;
crash-safe reconciliation of this preparation window remains a limitation.
