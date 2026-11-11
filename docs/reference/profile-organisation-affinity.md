---
title: Profile Organisation Affinity
description: Organisation restrictions and session warnings for automatic login swaps.
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
- [Swap record](#swap-record)
- [Session delivery](#session-delivery)
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
| `amplihack profile rotate --dry-run` | Previews selection under the affinity policy without moving a login or writing a swap record |
| `amplihack profile rotate` | Applies the policy, writes the warning to the rotate log and records the organisation outcome in the swap record |
| `amplihack profile rotate status` | Prints the last swap's warning (`last swap: ...`) when that swap was a crossing or unresolved |
| `amplihack profile auto-switch status` | Prints the last auto-switch crossing (`last crossing: ...`), with both profiles, both organisations and the warning |

Auto-switch makes its swaps inside the status-line wrapper that Claude Code
runs after each status-line update, not in any `profile auto-switch`
subcommand. When such a swap is a crossing or unresolved, the wrapper:

- appends `auto-switch in <dir> from <profile> to <profile>: <warning>` to
  the rotate log;
- writes `<profile-root>/.auto-switch.d/affinity/last-crossing.json` with
  `at`, `dir`, `from`, `to`, `from_org`, `to_org` and `affinity`, replacing
  any earlier record;
- writes the same line on its own stderr. Claude Code does not display a
  status-line command's stderr, so this copy is for manual runs only.

A swap within one organisation writes none of these.

For setup and command examples, see [Rotate Claude Logins](../howto/claude-profile-rotation.md)
and [Switch Accounts Automatically](../howto/claude-profiles.md#switch-accounts-automatically).

## Warnings

Rotate logs, `rotate status`, `auto-switch status` and session delivery use the same text:

| Outcome | Warning text |
| --- | --- |
| Crossing | Cross-organisation swap: earlier thinking becomes unavailable on the next turn. The visible conversation remains and the session can continue. |
| Unresolved | Organisation affinity is unresolved: earlier thinking may become unavailable on the next turn. The visible conversation remains and the session can continue. |
| Same | None |

The warning text contains no credentials, tokens, organisation IDs or
conversation content. The swap record and the `last crossing` line of
`auto-switch status` show organisation IDs; neither reaches a session.

## Swap record

The rotator's swap record (`<profile-root>/.auto-switch.d/rotate/swap.json`,
see [profile-command](profile-command.md)) carries three fields for this
feature:

| Field | Meaning |
| --- | --- |
| `from_org` | The outgoing login's organisation ID, or absent when unknown |
| `to_org` | The incoming login's organisation ID, or absent when unknown |
| `affinity` | `same`, `crossing` or `unresolved`, derived from the two IDs |

A record whose `affinity` disagrees with its IDs, or whose IDs are not valid,
is rejected as a whole. The record is overwritten by each swap; it describes
the latest swap only. The rotate log keeps the history.

## Session delivery

The warning reaches a session through the `limit-resume` hook, with the
resume that follows the swap. When the hook decides to resume a stopped
session, it reads the organisation outcome from the swap record that woke
it, and from nothing else:

- On the exit-2 wake, the hook writes the warning on stderr, on the line
  before the resume message. Claude Code passes exit-2 stderr to the
  session.
- On the tmux route, the warning text replaces the word `continue` in the
  keys it sends, followed by `Continue.`

A stop that does not end in a resume (rotator down, auto-switch off, no swap
with room, the session moved on) gets no warning. A session that keeps
running through a swap is not told by the hook; the swap is in the rotate log,
`rotate status` and, for an auto-switch swap, `auto-switch status`, and
Claude Code shows its own notice when it cannot use thinking from another
organisation.

The hook keeps no per-session warning state. Each call decides from the swap
record alone, so delivery does not depend on which process is the hook's
parent. Claude Code runs hook commands through `/bin/sh -c`; where `/bin/sh`
is dash, the shell does not `exec` the command, and the hook's parent is the
short-lived shell, not the Claude Code process. A design that identified the
session by the hook's parent would lose its state when the shell exits. The
test `shell_wrapped_hook_delivers_the_warning_from_the_swap_record_alone`
(`bins/amplihack-hooks/tests/limit_resume_binary.rs`) runs the real binary
through `/bin/sh -c` twice to cover this.

The same shell wrapper means the tmux route's pane check, which compares the
pane's terminal with the hook parent's standard input, does not match on
such hosts, and the hook falls back to the exit-2 wake. That behaviour
predates this feature; the warning goes out on whichever route the resume
takes.

Warning delivery does not change `judge()` or resume authorisation. A
failure to write the warning to stderr still exits 2. The tmux route sends
the fixed warning text with `send-keys -l` and never puts record contents
into a command.

Auto-switch does not write the rotator's swap record and does not wake
sessions. It prints the warning on its own stderr when it makes a crossing
or unresolved swap.

## Compatibility and platform limits

Legacy swap records remain readable. Absent organisation fields mean unknown
historical information, and a record without an `affinity` field produces no
warning. New records always carry `affinity`.

Affinity does not remove existing platform restrictions on rotation or
session discovery. See the rotation guide's [limits](../howto/claude-profile-rotation.md#limits).
Waiting for a swap and the tmux route are Linux-only. Restoring earlier
thinking and measuring the quality of later work are outside this feature.

### Selection rationale

Strict capacity relief and plan-room fallback use separate candidate pools.
The existing selectors define capacity using the configured thresholds; affinity
does not introduce a second percentage limit. Same-organisation room above a
threshold cannot hide a different organisation's eligible capacity relief.
When no strict relief target is selected, existing plan-room policy can still
choose an eligible, confirmed same-organisation target with room.
