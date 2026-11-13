---
title: Usage observation contract
description: Identity, ordering, storage and publication rules shared by usage producers and the rotator.
doc_type: reference
review_schedule: as-needed
---

# Usage observation contract

`amplihack_utils::usage_observation` provides the version-2 record contract
shared by the CLI and hooks. Records carry usage evidence for one validated
profile/account and login generation. They contain no credentials and use
private files rather than a database.

Complete-window validation and reset-aware comparison apply to both stored
observations and event-driven decisions.

See [event-driven rotation](../concepts/event-driven-rotation.md) for decision
behaviour and the [rotator guide](../howto/claude-profile-rotation.md) for commands.

## Contents

- [Shared interfaces](#shared-interfaces)
- [Record contents](#record-contents)
- [Window representation and validation](#window-representation-and-validation)
- [Reset-aware decisions](#reset-aware-decisions)
- [Identity and login generations](#identity-and-login-generations)
- [Capture and publication](#capture-and-publication)
- [Read and publication outcomes](#read-and-publication-outcomes)
- [Private storage](#private-storage)
- [Producer attribution](#producer-attribution)
- [Probe environment](#probe-environment)

## Shared interfaces

The shared Rust interfaces live in
[`amplihack_utils::usage_observation`](../../crates/amplihack-utils/src/usage_observation/mod.rs).
[`session_attribution`](../../crates/amplihack-utils/src/session_attribution.rs)
resolves immutable session ownership. These are internal integration
interfaces; observation files are not a public configuration format.

| Interface | Responsibility |
|-----------|----------------|
| `ObservationStore::open` | Open managed observation state beneath the configured profile root with private-storage checks. |
| `ObservationStore::lock` | Acquire the existing `.auto-switch.d/lock` and return an `ObservationGuard` for capture, validation and publication. |
| `ObservationStore::under_state_lock` | Return a guard when the caller already holds that same root's state lock. The caller retains the lock through validation and publication; this method does not acquire it. |
| `ObservationGuard::capture_new` | Allocate persistent ordering before collecting a new reading. Retain the returned `Capture` until completion. |
| `ObservationGuard::capture_once` | Reuse a named capture on redelivery without allocating another ticket. |
| `ObservationGuard::publish` | Validate evidence, current ownership, generation and ordering, then atomically replace the record. |
| `ObservationGuard::read` | Distinguish usable evidence from missing, stale, invalid, unsupported or unsafe records. |

A caller without the state lock uses `lock`; a caller inside a login or
swap transaction uses `under_state_lock`. Taking another lock inside the
transaction can deadlock. Collection happens outside the critical section;
publication rechecks ownership after reacquiring it.

## Record contents

| Information | Contract |
|-------------|----------|
| Schema version | Version 2; complete version-1 readings are accepted for migration. Other versions are rejected. |
| Owner | Validated profile and account identity, independent of a supplied display name. |
| Login generation | Non-secret binding established by an authorised login transaction. |
| Capture ticket | Monotonic within that generation, allocated when trusted measurement starts or an attributed event is accepted. |
| Observation identifier | Bound to that ticket; publishing the same observation again keeps its identifier and ticket. |
| Evidence | Session and weekly percentages, explicit reset values or unknown resets, and recognised limit state. Complete-window validation uses the representation below. |
| Capture time | When evidence was obtained; publication or rereading does not advance it. |
| Publication metadata | Accepted revision and ordering information; metadata changes alone do not start a policy decision. |

`Evidence` carries `session_window`, `session_used`, `week_used`,
`session_reset_at`, `week_reset_at` and `limit_reached`. An ordinary reading
requires the all-models weekly percentage and either a measured session
window or explicit confirmation that the session limit is absent.

Percentages must be finite numbers from 0 to 100 inclusive. Reset timestamps
are validated epoch seconds; an unknown reset is represented by `None`
(`null` in JSON), never a guessed reset. Missing windows do not become zero
usage or proof of unlimited allowance. A recognised limit is usable evidence
even when it supplies no percentages. It does not invent a session or weekly
percentage or establish capacity for ranking. A failed probe is unknown
usage, never evidence of exhaustion or available capacity.

Readers enforce byte limits before deserialisation, bound strings and
collections, reject invalid percentages and check timestamp and counter
arithmetic. A record that fails validation cannot authorise a swap. Records
are bounded at 16 KiB. Decision inputs are specified under [reset-aware decisions](#reset-aware-decisions).

## Window representation and validation

`Evidence::session_window` records explicit availability in schema version 2:

| Value | Meaning | Requirements |
|-------|---------|--------------|
| `present` | A session window exists. | A complete reading requires `session_used`; its reset may be unknown. |
| `absent` | A trusted producer explicitly confirmed no session limit. | `session_used` and `session_reset_at` must both be `None`. |
| `unknown` | The producer could not establish session-window availability. | No session percentage or reset; this state cannot establish capacity. |

A missing discriminator in a version-1 record with a session percentage
means present. Otherwise it means unknown, never absent.
Missing data, a failed probe and an omitted payload field cannot establish
`absent`. Probe parsers retain raw observation evidence before compatibility
display rules hide a session window. A measured zero remains a present
window with an unknown reset; a weekly-only text answer remains unknown.
Only an explicit null window in a trusted structured payload confirms
absence. Producers must retain the all-models weekly window, rather than
substituting a model-specific window.

Publication and read validation must enforce the same rules:

| Evidence | Required outcome |
|----------|------------------|
| Weekly percentage plus `present` session window with session percentage | Complete plan reading eligible for capacity ranking, subject to policy. |
| Weekly percentage plus explicitly `absent` session window | Complete plan reading eligible for capacity ranking without inventing session usage. |
| Missing weekly percentage, `present` without a session percentage, or missing/unknown session availability | Incomplete ordinary reading: reject publication as `InvalidEvidence`; read as `Invalid`. |
| Recognised `limit_reached` with incomplete windows | Accept as limit evidence only; cannot authorise a capacity-based choice. |
| Invalid percentage, reset or contradictory window fields | Reject even when `limit_reached` is true. |

Unknown resets do not invalidate an otherwise complete reading. A limit flag
does not supply missing windows or prove available capacity, even alongside
valid percentages. Existing percentage, timestamp, identity and ordering
checks still apply.

Publication, reads and ranking share this representation. New publications
use schema version 2. A version-1 ordinary record without both percentages
is invalid; a recognised limit remains limit evidence only.

## Reset-aware decisions

Both reset timestamps survive record-to-input conversion and participate in
semantic comparison alongside percentages, window availability and the limit
flag. A reset changing from known to unknown, unknown to known, or to a
different timestamp starts one decision. Duplicate notifications and capture
or revision metadata alone do not repeat that decision.

Passing a known reset time without a publication does not start work or clear
a limit flag. Unknown resets remain unknown; the watcher does not infer a
reset from elapsed time.

## Identity and login generations

Authorised login, replacement, adoption, removal and recovery transactions
establish or retire the generation. Re-login invalidates observations from
the previous generation, including when the profile name is unchanged.
Removing and recreating a profile cannot revive its previous observations.

Journalled credential movement and a known refresh preserve the account's
generation. Non-secret filesystem identity detects unexplained external
replacement and invalidates earlier evidence. It neither reads nor hashes
credential contents. Filesystem metadata alone cannot prove account identity
or authorise movement. Producers cannot create a missing binding to make
their payload acceptable.

Generation recovery is crash-consistent with login recovery. Recovery never
restores retired evidence, and a delayed completion cannot be relabelled with
the current generation.

Shared-path collection also captures a persistent movement counter. Moving a
login away and back invalidates that collection even when the original inode
returns. An authorised refresh receipt can preserve a collection across its
own token refresh without hiding a movement. Running sessions retain their
original logical owner and do not infer it from a later path occupant.

## Capture and publication

1. Resolve trusted attribution and capture the generation under the existing
   state lock. Allocate the generation-bound capture ticket there.
2. Measure without holding the lock. Keep the captured identity, generation,
   ticket and observation identifier with the result.
3. Reacquire the lock and validate the binding and ordering. Allocate the
   accepted revision and update the persistent accepted high-water mark as
   part of publication. Use the existing lock protocol without nested locking.
4. Publish the complete record by atomic replacement in the same verified
   directory. Report success before signalling probe completion.

If publication fails, the previous complete record is preserved. Completion
reports failure to the owning decision; it must not report usable new
evidence or leave the decision waiting forever.

Ticket allocation and ordering bookkeeping do not cause rotation decisions.
If ticket 12 is accepted before ticket 11 finishes, ticket 11 is superseded
and cannot overwrite the newer evidence. Repeating an identical accepted
publication returns `Duplicate`. Retrying a publication does not allocate a
new ticket. Reusing an accepted ticket or observation identifier with
different evidence is an error, never an overwrite. Ordering metadata and
the accepted record recover conservatively after interruption: evidence may
become unavailable, but a late older write cannot replace newer evidence.
Atomic replacement does not promise durability against every system crash.

## Read and publication outcomes

Reads distinguish usable evidence, a missing observation, an obsolete login
generation, an unsupported or invalid record, unsafe storage and an I/O
failure. Callers preserve those distinctions rather than treating every
failure as an empty record or an exhausted account.

Publication distinguishes acceptance, `Duplicate`, a superseded ticket,
a retired generation, invalid attribution, invalid evidence and storage
failure. A superseded or retired probe result is terminal for that job. It
does not trigger a replacement probe. Diagnostics omit credentials and raw
authentication payloads.

## Private storage

Records are stored at
`<profile-root>/.auto-switch.d/observations/<profile>.json`; binding and
ordering state is maintained by the shared store. Each observation belongs
to its validated profile's own state, even while the login is live in `~/.claude` or hosted in another profile. Producers do
not write to whichever account happens to occupy a directory at completion.
Observation files are managed state, not a user-editable configuration API.

Storage uses validated profile names and descriptor-relative operations in
verified directories. Opened objects are checked for effective ownership,
private permissions and type. Symlinks, non-regular files and unsafe
directories are refused. Temporary files are created exclusively with
private permissions and replaced atomically within the same directory.
Readers see a complete old or new record, never a partial write.

Observation storage and filesystem change detection neither read nor hash
credential contents. Existing authorised authentication handling remains
responsible for trusted identity checks and token refresh.

## Producer attribution

Session-start resolves a unique authorised credential location in the
generation store and persists its logical profile and generation for the
session. This works before a rotation ledger exists. Later status-line and
home-line publications use that captured binding, even after another login
occupies the directory. Payload account claims cannot establish ownership.
Ambiguous sessions keep their existing display and resume behaviour but do
not publish attributed observations.

Explicit usage collection captures ownership, location and ordering before
measurement, then validates them again before publication. Passive display
readings are not republished as new observations. A running rerun session
retains its original logical owner; an actual later limit event receives a
new capture ticket rather than reusing the session's earlier collection
order.

The actual limit hook publishes recognised, attributed limit evidence before
checking whether it can resume the session. Publication therefore still
works when the resume gate refuses, including when the rotator is down.
Publishing evidence does not bypass process verification, duplicate-resume
protection, resume caps, tmux checks or swap-record validation.

## Probe environment

All Claude usage-probe paths set `AMPLIHACK_USAGE_PROBE=1` in common command
preparation before sandbox wrapping: structured `get_usage`, text `/usage`
and fallback invocations. Only the exact marker value `1` suppresses
applicable amplihack session hooks. Probe session-start returns before expensive amplihack initialisation;
other marker values do not suppress it. The marker is a performance control,
not identity or authorisation evidence.

Ordinary interactive, profile, worker, login, recipe-worker and LLM command
builders clear an inherited probe marker so their full session-start
behaviour runs. No user-wide hook setting is changed. Claude probes retain `--no-session-persistence` and the
Linux bwrap protection over the shared projects directory, with existing
fallback behaviour when bwrap is unavailable. Every fallback retains the
marker and persistence flag. A fallback that cannot retain required
isolation fails terminally; it cannot silently launch an ordinary session.
