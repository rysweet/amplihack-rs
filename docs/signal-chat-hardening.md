# Signal Chat Hardening

Hardening reference for the Signal chat feature that spans
[`crates/amplihack-signal`](../crates/amplihack-signal) and
[`crates/amplihack-cli`](../crates/amplihack-cli). It documents six
review-feedback fixes — a consolidated loopback endpoint validator (**F1**), a
fail-closed group-membership parser (**F3**), and a race-free child
pre-emption mechanism (**F2**) that replaces the former raw-PID SIGKILL and its
PID-reuse TOCTOU window, plus a second pre-merge pass that closes the
receive-path and authorization gaps surfaced in review: a cancel-safe inbound
receive path (**F4**) that can never silently drop a fragmented operator
message, E.164-validated group membership (**F5**) that rejects any member
without a well-formed number, and per-post membership re-verification (**F6**)
that re-authorizes the group before **every** outbound chunk.

Everything described here is compiled only under the `signal` Cargo feature. The
crate compiles cleanly both feature-on and feature-off; with the feature off,
none of the items below are present.

> **Status — forward specification.** This document describes the *intended*
> hardened state. The items it hardens (the `amplihack-signal` `chat/` module,
> `transport::parse_group_members`, and the CLI `preempt_child`) land with the
> first version of the Signal chat feature (issue #1054) and are **not present
> on a branch cut from `main`**. Until this work is rebased onto that base, treat
> the present-tense descriptions below as the target contract for the F1/F2/F3
> changes rather than as shipped behavior. The single item that already exists
> today is the CLI `signal::validate::validate_loopback_endpoint`, which F1
> reduces to a delegate.
>
> **Second pass (F4/F5/F6).** The receive-path and authorization fixes below
> land as pre-merge hardening on the same consolidated branch
> (`feat/signal-phase-a-consolidated`, PR #1065). They build on F3's fail-closed
> membership parse and the existing `SignalTransport` framing; treat their
> present-tense descriptions as the target contract for that PR.

Read this document when you need to:

- validate a remote relay endpoint and understand which hosts/ports are
  accepted,
- reason about why a Signal group membership is classified `Unverified` and the
  relay withheld,
- audit the race-free child pre-emption mechanism (owned-`Child` `start_kill`).

---

## Contents

- [Overview](#overview)
- [F1 — Canonical loopback endpoint validator](#f1--canonical-loopback-endpoint-validator)
  - [`validate_loopback_endpoint`](#validate_loopback_endpoint)
  - [Acceptance / rejection matrix](#acceptance--rejection-matrix)
  - [`chat::validate_endpoint` delegation](#chatvalidate_endpoint-delegation)
  - [CLI delegation](#cli-delegation)
- [F3 — Fail-closed membership parse](#f3--fail-closed-membership-parse)
- [F2 — Child pre-emption (race-free, owned-`Child` kill)](#f2--child-pre-emption-pid-reuse-toctou-fixed)
- [F4 — Cancel-safe inbound receive](#f4--cancel-safe-inbound-receive)
- [F5 — E.164-validated group membership](#f5--e164-validated-group-membership)
- [F6 — Per-post membership re-verification](#f6--per-post-membership-re-verification)
- [F7 — Auto-accept code-created groups](#f7--auto-accept-code-created-groups-reliable-delivery)
- [F8 — Broadened outbound secret redaction](#f8--broadened-outbound-secret-redaction)
  - [`redact_for_relay`](#redact_for_relay)
  - [The broadened credential-assignment rule](#the-broadened-credential-assignment-rule)
  - [What is caught now (and what is not)](#what-is-caught-now-and-what-is-not)
  - [Idempotency](#idempotency)
  - [Bounded over-redaction](#bounded-over-redaction)
- [Security invariants](#security-invariants)
- [Exit-code taxonomy](#exit-code-taxonomy)
- [Testing](#testing)
- [See also](#see-also)

---

## Overview

The Signal chat relays messages between a Signal group and a local
Copilot/agent runtime. Three trust boundaries are hardened here:

1. **Network egress** — the chat will only dial a **loopback** relay endpoint
   unless an operator explicitly opts into an unsafe remote. Prior to this pass
   two validators disagreed on what "loopback" meant; F1 makes a single
   canonical validator the source of truth.
2. **Authorization** — a message is only relayed to a Signal group whose
   membership can be positively verified against E.164 numbers. F3 ensures a
   member with a missing/unparseable number can never be silently dropped from
   that check; **F5** tightens this further by rejecting any member whose
   `number` is present but **not a well-formed E.164 value**; **F6** re-runs the
   check before **every** outbound chunk so a mid-body membership change cannot
   receive later chunks.
3. **Inbound framing** — an operator message arriving over the signal-cli
   JSON-RPC socket may be split across multiple TCP segments. **F4** makes the
   inbound receive path **cancel-safe** so a partially-read frame is never lost
   when the subscriber loop's `select!` wakes on a competing event, and a
   notification that arrives while a JSON-RPC `request` is in flight is
   **queued, never discarded**.

All three boundaries are **fail-closed**: any ambiguous, unparseable, missing,
or fragmented input results in rejection (endpoint), `Unverified`
classification (membership), or safe retention (inbound frame) — never a
default-allow and never a silent drop.

A fourth, delivery-side invariant is hardened alongside them:

4. **Delivery reliability** — a group the chat *creates* is delivered to the
   operator's linked device as a **pending message request** and may silently
   withhold messages until accepted. **F7** bakes acceptance into group creation
   (`create_group` auto-accepts via `sendMessageRequestResponse`), and does so
   **fail-closed**: if acceptance fails, group creation fails rather than leaving
   a group whose messages never reach the operator.

---

## F1 — Canonical loopback endpoint validator

Before this pass there were **two** divergent validators:

| Location | Behavior |
| --- | --- |
| `amplihack_signal::chat::validate_endpoint` (runtime) | bespoke host/port split; **false-rejected** the bare IPv6 loopback `::1:9000` |
| `amplihack_cli` `signal::validate::validate_loopback_endpoint` (CLI) | correct `rsplit_once(':')` split; already **accepts** bare `::1:9000`, rejects port `0`/out-of-range/wildcard host |

They are now consolidated into a **single canonical, lexical-only** validator
that lives in the signal crate (dependency direction is CLI → signal, so the
canonical implementation is hoisted *down* into the signal crate). The canonical
validator adopts the CLI's existing "last-colon-wins" semantics verbatim; the
runtime and the CLI both delegate to it. Net validator LOC drops.

The only intended behavior change is on the **runtime `chat::validate_endpoint`
path**, which previously false-rejected the bare, bracket-less IPv6 loopback
`::1:9000` and now **ACCEPTS** it (matching the CLI). The **CLI path is fully
behavior-preserving** — it already accepted bare `::1` and rejected zero/wildcard
ports, so the consolidation only removes its now-duplicate `split_host_port`
helper without changing any accept/reject outcome. Everything else on both paths
is unchanged.

### `validate_loopback_endpoint`

```rust
// crate: amplihack-signal  (feature = "signal")
use amplihack_signal::chat::{validate_loopback_endpoint, EndpointError};

// OK — loopback host + valid port
validate_loopback_endpoint("127.0.0.1:8080")?;
validate_loopback_endpoint("localhost:443")?;
validate_loopback_endpoint("[::1]:9000")?;   // bracketed IPv6 loopback
validate_loopback_endpoint("::1:9000")?;      // bare IPv6 loopback (now accepted)

// Err(EndpointError) — see rejection rules below
assert!(validate_loopback_endpoint("0.0.0.0:8080").is_err());
assert!(validate_loopback_endpoint("example.com:443").is_err());
assert!(validate_loopback_endpoint("127.0.0.1:0").is_err());
```

Signature:

```rust
pub fn validate_loopback_endpoint(endpoint: &str) -> Result<(), EndpointError>;
```

Properties:

- **Lexical / numeric only.** The validator performs **no DNS resolution** and
  never calls `to_socket_addrs`. Only the literal label `localhost` is treated
  as a loopback name; every other name is rejected. This closes the
  DNS-rebinding TOCTOU class by construction.
- **Fail-closed.** Any parse failure returns `Err(EndpointError)`. There is no
  branch that defaults to acceptance.
- `EndpointError` is a `thiserror` enum whose `Display` messages reference the
  *defect* (e.g. non-loopback host, invalid port) and never embed a resolved
  address or other value.

### Acceptance / rejection matrix

| Endpoint | Result | Reason |
| --- | --- | --- |
| `127.0.0.1:8080` | ✅ accept | IPv4 loopback |
| `127.0.0.5:1234` | ✅ accept | anything in `127.0.0.0/8` |
| `localhost:8080` | ✅ accept | literal loopback label |
| `[::1]:9000` | ✅ accept | bracketed IPv6 loopback |
| `::1:9000` | ✅ accept | **bare IPv6 loopback (F1: unblocks the runtime path; already accepted by the CLI)** |
| `0.0.0.0:8080` | ❌ reject | wildcard host |
| `::` / `[::]:8080` | ❌ reject | IPv6 unspecified/wildcard |
| `10.0.0.5:8080` | ❌ reject | routable host |
| `example.com:443` | ❌ reject | DNS name (no resolution performed) |
| `127.0.0.1:0` | ❌ reject | port 0 |
| `127.0.0.1:70000` | ❌ reject | port > 65535 |
| `127.0.0.1` | ❌ reject | missing port |

Valid ports are `1..=65535`. Wildcard hosts (`0.0.0.0`, `::`), embedded-IPv4
forms, and every non-`localhost` DNS name are rejected.

**Host/port split rule.** A bracketed input (`[host]:port`) splits on the
literal `]:`. Every other input splits on its **last** colon ("last-colon-wins"),
so the substring after the final `:` is the port and everything before it is the
host. This is what lets the bare, bracket-less IPv6 loopback parse as
host `::1` + port `9000` for `::1:9000`. Note the deliberate trade-off: a bare
`::1:9000` is *also* a syntactically valid IPv6 literal
(`0:0:0:0:0:0:1:9000`); the validator resolves this ambiguity in favor of the
`host:port` reading. Callers that need an unambiguous IPv6 form should prefer the
bracketed `[::1]:9000`. A bare `::1` with no port is rejected (no port
component), consistent with the missing-port row above.

### `chat::validate_endpoint` delegation

The runtime entry point keeps its unsafe-remote short-circuit and then delegates:

```rust
// crate: amplihack-signal  (feature = "signal")
pub fn validate_endpoint(endpoint: &str, unsafe_remote: bool)
    -> Result<(), ChatError>
{
    // Explicit operator opt-in bypasses loopback enforcement.
    if unsafe_remote {
        return Ok(());
    }
    // Single source of truth; any failure maps to a rejection.
    validate_loopback_endpoint(endpoint)
        .map_err(|_| ChatError::RemoteEndpointRejected)
}
```

- `unsafe_remote = true` remains the **only** non-loopback path. With it set,
  routable endpoints such as `10.0.0.5:8080` are accepted.
- All rejections surface as `ChatError::RemoteEndpointRejected` (exit code
  `2` — see [Exit-code taxonomy](#exit-code-taxonomy)). No new `ChatError`
  variants and no new success paths were added.
- The previous bespoke `is_loopback_host` helper and inline host/port splitting
  are deleted.

### CLI delegation

The CLI keeps its public function name and signature so callers are untouched;
it becomes a thin `anyhow`-wrapping delegate to the canonical validator:

```rust
// crate: amplihack-cli  (feature = "signal")
pub fn validate_loopback_endpoint(endpoint: &str) -> anyhow::Result<()> {
    amplihack_signal::chat::validate_loopback_endpoint(endpoint)
        .map_err(anyhow::Error::from)
}
```

The CLI's bespoke `split_host_port` helper is deleted. A parity test asserts the
CLI delegate and the canonical validator agree on the full matrix and message
surface.

---

## F3 — Fail-closed membership parse

`amplihack_signal::transport::parse_group_members` builds the list of E.164
member numbers used to authorize a relay. Previously it used a `filter_map`
that **silently dropped** any member lacking a string `number` field — a
fail-open behavior that could shrink the verified set and admit a relay to a
group whose membership was not fully verified.

It is now **fail-closed**: if *any* member payload lacks a valid string
`number`, the whole parse returns `Err(WireError::Membership(..))`. F3 adds the
`Membership` variant to the `WireError` enum. Today's enum on this branch carries
only the `Json` variant; the first version of the feature adds the frame/transport variants that
`parse_group_members` needs, and F3 adds `Membership` on top of those. Its
message is a fixed, PII-free string that names the defect and interpolates no
member value.

```rust
// crate: amplihack-signal  (feature = "signal")
// A member missing the E.164 `number` field is a parse FAILURE, not a skip.
let members = parse_group_members(&payload)?; // Err(WireError::Membership) if any member is invalid
```

Downstream effect:

```
parse_group_members(..) == Err
        └─▶ group_members == None
                └─▶ classify(None) == Membership::Unverified
                        └─▶ relay is WITHHELD
```

Guarantees:

- **Never silently drops a member.** A missing/non-string `number` is treated
  as a mismatch that fails the entire parse.
- The resulting classification is `Membership::Unverified`, so no partial relay
  is delivered to a mixed/unverifiable set.
- **PII-safe:** the `WireError::Membership` message references the defect (a
  member missing its number) and never embeds the raw phone number.

A unit test asserts that a member payload missing `number` classifies as
`Membership::Unverified` and the relay is withheld.

---

## F2 — Child pre-emption PID-reuse TOCTOU (**fixed**)

`preempt_child` (CLI signal chat) pre-empts an in-flight Copilot turn so a
control `stop`/`kill` from the operator group can terminate the running child.

**Previous design (removed).** The old implementation stored the child's **raw
PID** in a shared `Arc<Mutex<Option<u32>>>` slot and issued
`unsafe { libc::kill(pid, SIGKILL) }`. Because it operated on a raw PID rather
than an owned `Child` handle, it had a time-of-check/time-of-use window: between
the turn task reaping the child (freeing the PID) and clearing the slot, the OS
could recycle the PID, so the signal could be delivered to an unrelated process.

**Current design (race-free).** Pre-emption is now bound to the **specific owned
child**, immune to PID reuse. The raw-PID slot and the `libc::kill` path are
deleted entirely:

- `chat::turn` exports a `PreemptSlot = Arc<Mutex<Option<oneshot::Sender<()>>>>`
  type alias. `CopilotTurnRunner::new(program, preempt)` takes this slot.
- In `run_argv`, the runner spawns the child, publishes a `oneshot::Sender<()>`
  (a pre-empt trigger bound to *that* child) into the slot, and takes the
  child's stdout/stderr pipes, draining them concurrently so a full pipe can
  never deadlock the reap.
- It then `tokio::select!`s between `child.wait()` and the paired oneshot
  receiver. If the receiver fires first, it calls `child.start_kill()` on the
  **owned** [`tokio::process::Child`] handle — which the runtime binds to that
  exact process — then reaps via `child.wait().await`, and surfaces the turn as
  `io::ErrorKind::Interrupted` ("turn pre-empted by stop").
- On any completion the runner clears the slot, so a later pre-empt is a
  harmless no-op.
- `preempt_child` simply takes the sender out of the `PreemptSlot` and
  `send(())`s it — no raw PID, no `libc::kill`, no TOCTOU window.

This is covered by dedicated tests (a long-blocking child pre-empted mid-turn
resolves to `Interrupted`; a normal turn still returns its stdout).

**Residual (R1, Low).** The concurrent drain uses `read_to_end`, so a turn's
stdout/stderr is buffered **unbounded** in memory. The child is trusted
(operator-launched `copilot`), so this is accepted; add an output cap here if
that trust boundary ever changes.

---

## F4 — Cancel-safe inbound receive

`amplihack_signal::transport::SignalTransport::receive` reads one Signal
`Envelope` per call from the signal-cli JSON-RPC socket. In
`crates/amplihack-cli/src/commands/signal/chat.rs` the subscriber loop polls
`receive()` as one arm of a **`biased` `tokio::select!`**, racing it against the
turn/queue channel. That makes `receive()` a **cancellation point**: whenever a
competing arm wins, the in-flight `receive()` future is **dropped**.

**Previous defect (silent inbound frame loss).** The old `read_line` cleared its
persistent `raw_buf`/`line_buf` at the **top of every call** and called
`reader.consume(..)` on partial chunks **before** seeing the terminating
newline. A frame delivered across several TCP segments was therefore
accumulated across multiple `fill_buf`/`consume` iterations. If the `select!`
dropped the `receive()` future mid-frame, the already-`consume()`d prefix bytes
were gone from the reader **and** the next call cleared `raw_buf` — silently
discarding one inbound operator message (e.g. a large pasted prompt fragmented
across segments). This violated the chat's core **"never silent"** promise.

**Current design (cancel-safe, single-reader, no mpsc).** The receive path is
made cancel-safe **within the transport**, preserving the existing single-task,
single-`&mut transport` model and the public `receive()` signature. No dedicated
reader task and no `mpsc` channel are introduced — a second reader on the same
socket would starve the JSON-RPC `request` path, and reworking response
correlation is out of scope for this hardening pass.

- **Persistent partial-frame state.** `read_line` **no longer clears** its
  accumulation buffer on entry. In-progress frame bytes (and the
  oversized-drain latch) persist across calls in additive **private** struct
  fields. The single `.await` (`fill_buf`) remains the **only** cancellation
  point, so a future dropped mid-frame resumes on the next call with the
  accumulated bytes **intact** — no loss, no duplication. The buffer is reset
  **only after** a complete frame (or a fully-drained oversized frame) is
  returned.
- **Preserved bound.** The [`MAX_FRAME_BYTES`](../crates/amplihack-signal/src/transport.rs)
  (256 KiB) cap is still enforced **on the persisted buffer**, so an attacker
  streaming a giant frame one segment at a time cannot grow memory without
  bound. The oversized-drain and empty-line resync behavior are unchanged.
- **Notification queue (`pending_incoming`).** The JSON-RPC `request` path
  (`group_members`, `send_group`, `create_group`, `quit_group`) reads frames
  until it sees its response `id`, and previously **discarded** any interleaved
  notification. It now **pushes each parsed notification `Envelope` onto an
  in-memory `VecDeque` (`pending_incoming`)** instead of dropping it. `receive()`
  **drains `pending_incoming` first**, before reading the socket, so a
  notification that arrived while a `request` was in flight is delivered on the
  next `receive()` — queued, never lost.

Together these guarantee **exactly-once, never-dropped** notification delivery
across fragmentation, mid-frame cancellation, **and** interleaved `request()`
calls. Every drained `Envelope` — including those pulled from `pending_incoming`
— still flows through `Gate::evaluate` downstream, so the cancel-safety rework
introduces **no authorization bypass** (an empty allowlist still denies all).

```rust
// crate: amplihack-signal  (feature = "signal")
// A frame split across TCP segments and interrupted by a competing select
// event is resumed intact on the next call — no bytes are lost.
let env = transport.receive().await?; // drains pending_incoming first, then the socket
```

A regression test (`tests/transport_cancel_safe_it.rs`) uses the transport's
real-`TcpListener` chunked-write seam to deliver one inbound frame in multiple
TCP segments while a competing event drops the `receive()` future mid-frame and
an intervening `request()`/`group_members` call runs; it asserts the fragmented
`Envelope` is ultimately delivered **intact and exactly once**.

---

## F5 — E.164-validated group membership

F3 (above) made `parse_group_members` fail-closed on a member **missing** its
`number` field. F5 tightens the same loop: a member whose `number` is
**present but malformed** (empty, or not a well-formed E.164 value) must now
**fail the whole parse** as well — a non-conforming number can no longer slip
into the verified set.

- **Shared, in-crate predicate.** The check reuses the crate's existing
  `validate_e164` predicate (`+` followed by **1..=15 ASCII digits**) from
  [`config::resolver`](../crates/amplihack-signal/src/config/resolver.rs). Its
  visibility is promoted from `pub(super)` to **`pub(crate)`** so
  `parse_group_members` can call it directly. It is **not** imported from
  `amplihack-cli` (the dependency direction is CLI → signal; importing upward
  would create a circular dependency). The CLI validator in
  `signal::validate` uses the identical rule, so both paths agree by
  construction.
- **First-invalid rejection, fail-closed.** In the member loop, the **first**
  empty or non-conforming number returns the existing
  `Err(WireError::Membership(..))` — no new error variant is added. Downstream,
  this yields `group_members == None` → `Membership::Unverified` → the relay is
  **withheld**.
- **PII-safe.** The `WireError::Membership` message names the **defect** only
  and interpolates **no** phone number, so the existing
  `parse_failure_message_does_not_leak_member_numbers` regression test still
  passes.

```rust
// crate: amplihack-signal  (feature = "signal")
// Any member whose `number` is empty or not a valid E.164 value fails the parse.
let members = parse_group_members(&payload, group_id)?; // Err(WireError::Membership) on first malformed number
```

Membership tests are extended to cover both an **empty** number and a
**malformed** number (e.g. `+` with too many digits, or non-digit characters);
each rejects the **entire** parse and withholds the relay.

---

## F6 — Per-post membership re-verification

`verify_and_post` in
[`crates/amplihack-cli/src/commands/signal/chat.rs`](../crates/amplihack-cli/src/commands/signal/chat.rs)
relays a redacted agent reply that has been **chunked** to Signal's per-message
limit. The security posture states verification happens **"before EVERY
post"** — but the previous implementation verified membership **once per body**
and then sent all chunks. A member added **mid-body** (after the first
verification but before the last chunk) could therefore still receive the
remaining chunks.

**Current design (re-verify before each chunk).** Group membership is now
re-verified **immediately before EACH `send_group` chunk call**, not once per
body:

- Before each chunk, the runner re-runs `transport.group_members()` and
  `classify()` on the freshly-returned set.
- On a `Verified` result, the redacted chunk is sent verbatim via
  `send_group(group_id, &chunk)`. (This outbound agent-reply path carries no
  inbound `source`/`source_device`; those fields exist only on inbound
  `Envelope`s consumed by the `Gate`, so there is nothing to "preserve" here.)
- On **any** verification failure mid-body (error, timeout, or a now-`Unverified`
  set), the runner **stops sending the remaining chunks**, logs the withheld
  relay via the existing `WITHHOLDING` terminal notice (surfaced, **never a
  silent drop**), and returns fail-closed.

No message caps or fixed limits are introduced; the only cost is one additional
`group_members` round-trip per chunk, which is accepted for correctness.

```
verify_and_post(body):
    for chunk in chunk(body):
        members = transport.group_members()          # re-fetched per chunk
        if classify(members) != Verified:
            log "WITHHOLDING remaining N chunks"      # surfaced, fail-closed
            return                                    # no further chunks sent
        transport.send_group(chunk)                   # redacted chunk text only
```

An integration test (`crates/amplihack-cli/tests/signal_chat_it.rs`) proves
that a member removed/altered **between chunks** stops the subsequent chunks and
logs the withhold.

---

## F7 — Auto-accept code-created groups (reliable delivery)

`create_group` in
[`crates/amplihack-signal/src/transport.rs`](../crates/amplihack-signal/src/transport.rs)
originates a Signal group on behalf of the operator via signal-cli. A group that
**code** creates arrives on the operator's *linked* device as a **pending
message request**, not an open conversation. While pending, Signal may
**withhold or delay** messages the group posts to the operator — so the chat's
first announcement (topic, allowlist, control phrases) and early agent replies
could **silently fail to reach the phone** even though every relay path above is
correct. Delivery is a trust-adjacent invariant: the operator must actually see
what the driven agent says.

**Current design (accept baked into creation).** `create_group` accepts the new
group's message request **immediately after** the `GroupId` is known and
**before** returning it to any caller:

- After `updateGroup` returns the new `groupId`, `create_group` calls
  `accept_group(&gid)` and only then returns the id.
- `accept_group` issues signal-cli's `sendMessageRequestResponse` JSON-RPC with
  `{"groupId": "<gid>", "type": "accept"}` (a `{"result":{}}` success).
- Because the accept lives inside `create_group`, **every** caller that creates a
  group through the shared transport gets auto-accept for free — there is no flag
  and no opt-out (a group the chat cannot deliver to is not usable).

**Fail-closed.** Acceptance is **not** best-effort. If `accept_group` errors
(daemon error, timeout, RPC failure), `create_group` **propagates the error** and
group creation fails (surfaced to the operator, exit `3`) rather than returning a
group whose messages might never arrive. The chat never leaves a group in a
pending, silently-undelivered state — consistent with the no-silent-degradation
policy that governs the rest of this pass.

```
create_group(name):
    gid = updateGroup(name).groupId         # signal-cli create-by-name
    accept_group(gid)                        # sendMessageRequestResponse:accept
    #  └─ on error: propagate → create_group fails (fail-closed, exit 3)
    return gid                                # only a delivered-capable group escapes
```

A transport test drives `create_group` against the fake signal-cli endpoint and
asserts a `sendMessageRequestResponse` accept is issued for the newly created
group id (the fake records accepted group ids and exposes them via
`accepted_groups()`).

---

## F8 — Broadened outbound secret redaction

`redact_for_relay` in
[`crates/amplihack-signal/src/chat/outbound.rs`](../crates/amplihack-signal/src/chat/outbound.rs)
scrubs high-frequency secret shapes out of an agent reply **before** the body is
chunked and relayed to Signal (see [F6](#f6--per-post-membership-re-verification)
for the chunking/re-verification path). It is a **defense-in-depth** control: the
primary leak gate is the fail-closed group-membership check (F3/F5/F6). The
redactor is the second line — it assumes a body may still contain a pasted or
echoed credential and removes the common shapes deterministically.

This pass hardened the **generic credential-assignment rule** — the `name: value`
/ `name = value` pattern anchored on a secret keyword (`api_key`, `access_key`,
`secret`, `token`, `password`, `passwd`, `pwd`, `credential`, `authorization`).
Two gaps were closed:

1. **Short values slipped through.** The value match previously required at least
   six characters (`{6,}`), so a keyword followed by a short value
   (`token=x`, `password: abc`) was **not** redacted.
2. **Unusual charsets slipped through.** The value character class was
   `[A-Za-z0-9._~+/=:-]`, which does not include punctuation such as
   `! @ # $ % ^ & * ( )`. A punctuation-heavy secret
   (`api_key: a!b#c$d%e^f&g`) was therefore left in the clear.

Only this one rule changed. Every other pattern — PEM private-key blocks, Signal
device-link URIs, GitHub tokens, AWS access-key IDs, Google API keys, URL
userinfo passwords, Slack tokens, and the standalone HTTP `Bearer` rule — is
unchanged.

### `redact_for_relay`

```rust
/// Scrub high-frequency secret shapes out of `body`. Pure, idempotent, and
/// allocation-light (only adopts a new buffer on a real match).
pub fn redact_for_relay(body: &str) -> String
```

- **Pure and deterministic.** Same input ⇒ same output; no I/O, no clock, no
  randomness.
- **Allocation-light.** Returns the input unchanged (no new buffer) when nothing
  matches.
- **Whole-body first.** The chat always calls `redact_and_chunk`, which runs
  `redact_for_relay` over the **entire** body and only then splits into
  Signal-sized chunks, so a secret can never straddle a chunk boundary and
  survive.

### The broadened credential-assignment rule

The value match now accepts **one or more** non-whitespace, non-quote characters
(`[^\s'"]+`) instead of six-or-more characters drawn from a narrow class. In
plain terms: after the keyword and the `:`/`=`, an optional opening quote, and an
optional HTTP auth-scheme word (`Bearer`/`Basic`/`Token`), everything up to the
next whitespace or quote is treated as the secret and replaced.

| Aspect | Before | After |
| --- | --- | --- |
| Minimum value length | 6 characters | 1 character (never empty) |
| Value character set | `[A-Za-z0-9._~+/=:-]` | `[^\s'"]` (any non-space, non-quote) |
| Stops at | end of narrow-class run | first whitespace or quote |
| Keyword anchor | required | required (unchanged) |
| Case-insensitive (`(?i)`) | yes | yes (unchanged) |
| Scheme consumption `(?:(?:bearer\|basic\|token)\s+)?` | yes | yes (unchanged) |
| Surrounding quotes | optional | optional (unchanged) |
| Replacement | `$1=[REDACTED]` | `$1=[REDACTED]` (unchanged) |

The keyword anchor is what keeps this safe: the value is only broadened **after**
a recognized secret keyword and its `:`/`=`. A short or punctuation-heavy token
sitting immediately after a secret keyword is still a secret, so widening the
value match moves the control in the fail-safe direction.

### What is caught now (and what is not)

Redacted (each becomes `<keyword>=[REDACTED]`):

```text
token=x                        →  token=[REDACTED]
password: abc                  →  password=[REDACTED]
api_key: a!b#c$d%e^f&g         →  api_key=[REDACTED]
secret = example!#%notreal            →  secret=[REDACTED]
password="example!%"               →  password=[REDACTED]
```

Left unchanged (no secret keyword present):

```text
the meeting is at 3pm, see you there   →   (returned verbatim)
```

The value run stops at the first whitespace or quote, so the rule redacts the
**single** token after the keyword and never swallows the rest of the line.

### Idempotency

`redact_for_relay` is idempotent — running it twice yields the same result as
running it once:

```text
redact_for_relay(redact_for_relay(x)) == redact_for_relay(x)   // for all x
```

This holds for the broadened rule because the placeholder value `[REDACTED]` is
itself a run of non-whitespace, non-quote characters, so a second pass matches
`<keyword>=[REDACTED]` and re-emits the identical string. An idempotency test
mixes several secret shapes in one body and asserts one pass equals two.

### Bounded over-redaction

The broadening admits a small, accepted trade-off: a benign word placed
immediately after a secret keyword is over-redacted (only that one adjacent
token, never the rest of the line):

```text
password: is required   →   password=[REDACTED] required
```

The ` required` tail is preserved. For a scrubber this over-redaction is
acceptable — it errs toward removing rather than leaking — and the keyword anchor
keeps it from touching ordinary prose that has no secret keyword.

---

- **Single source of truth.** After F1 exactly one host/port parser exists in
  the workspace. A reappearing second parser is a security regression
  (validator divergence = confinement bypass).
- **No DNS in validators.** Endpoint validation is purely lexical/numeric; only
  the literal `localhost` label is accepted. This prevents DNS-rebinding TOCTOU.
- **Strict IPv6.** `::`, `[::]`, and embedded-IPv4 forms are rejected; only
  `::1` (bare or bracketed) is accepted as loopback.
- **Ports.** Port `0` and ports `> 65535` are rejected explicitly.
- **`unsafe_remote` is the only non-loopback path.** No implicit bypasses.
- **Fail-closed authorization.** Any member lacking a verifiable string
  `number` ⇒ `Membership::Unverified` ⇒ relay withheld; no partial relay to a
  mixed set.
- **E.164-validated membership.** A member whose `number` is present but empty
  or not a well-formed E.164 value (`+` then 1..=15 ASCII digits) fails the
  entire parse (`WireError::Membership`) ⇒ `Unverified` ⇒ withheld. The
  predicate lives once in the signal crate (`config::resolver::validate_e164`,
  `pub(crate)`); the CLI must never be imported upward to obtain it.
- **Re-verify before every post.** Membership is re-checked before **each**
  outbound chunk, not once per body; a mid-body membership change withholds all
  remaining chunks and logs the withhold (fail-closed, never silent).
- **Cancel-safe inbound framing.** The inbound receive path never loses a
  fragmented frame across a `select!` cancellation, and never discards a
  notification interleaved with a JSON-RPC `request` — such notifications are
  queued in `pending_incoming` and delivered on the next `receive()`. Every
  delivered `Envelope` still passes `Gate::evaluate` (no authorization bypass).
  `MAX_FRAME_BYTES` (256 KiB) stays enforced on the persisted partial-frame
  buffer, so a segmented oversized frame cannot exhaust memory.
- **PII discipline.** Neither `EndpointError` nor `WireError` `Display` embeds a
  resolved address or phone number — they reference the defect, not the value.
- **Reliable delivery, fail-closed.** A code-created group is auto-accepted
  (`sendMessageRequestResponse:accept`) inside `create_group` before the id is
  returned, so no chat can operate on a group whose message request is still
  pending. A failed accept **fails group creation** (propagated, exit `3`) — the
  chat never leaves a group in a pending, silently-undelivered state.
- **Defense-in-depth redaction, fail-safe.** `redact_for_relay` scrubs common
  secret shapes from the whole body before chunking. The generic
  credential-assignment rule matches a secret keyword followed by **any**
  non-whitespace, non-quote value of length ≥ 1, so short and punctuation-heavy
  secrets are caught. It is the **secondary** control (membership verification is
  primary), deterministic, and **idempotent** — applying it twice equals once,
  because the `[REDACTED]` placeholder re-matches and re-emits unchanged. Minor
  over-redaction of a single token adjacent to a keyword is accepted; the rule
  stops at the next whitespace/quote and never rewrites keyword-free prose.

---

## Exit-code taxonomy

| Condition | Error | Exit code |
| --- | --- | --- |
| Endpoint rejected (non-loopback, bad port, DNS name, missing port) | `ChatError::RemoteEndpointRejected` | `2` |

F1 preserves the existing taxonomy: every endpoint rejection continues to map to
`RemoteEndpointRejected` / exit `2`. No new codes were introduced.

---

## Testing

Validation gate for the hardening pass (all `signal`-feature-gated):

```bash
cargo fmt --all
cargo clippy -p amplihack-signal --features signal --all-targets -- -D warnings
cargo clippy -p amplihack-cli --features amplihack-cli/signal --all-targets -- -D warnings
cargo test  -p amplihack-signal --features signal            # incl. transport_cancel_safe_it
cargo test  -p amplihack-cli --test signal_chat_it --test signal_validator_parity
cargo build -p amplihack-signal            # feature-off compile check
```

Test coverage:

- **Endpoint matrix** (`amplihack-signal` unit tests): bare and bracketed `::1`
  accepted; `0.0.0.0`, `::`, routable hosts, DNS names, port `0`, and
  out-of-range ports rejected; `10.0.0.5` accepted when `unsafe_remote = true`.
- **CLI parity** (`signal_validator_parity`): the CLI delegate and the canonical
  validator agree on behavior and message surface.
- **Membership fail-closed** (`amplihack-signal` unit tests): a member payload
  missing `number` classifies as `Membership::Unverified` and the relay is
  withheld.
- **E.164 membership** (`amplihack-signal` tests): a member whose `number` is
  empty or malformed fails the entire `parse_group_members` (`WireError::Membership`)
  and withholds the relay; `parse_failure_message_does_not_leak_member_numbers`
  still passes (no phone number in the message).
- **Cancel-safe receive** (`transport_cancel_safe_it.rs`): a frame delivered in
  multiple TCP segments, interrupted by a competing `select!` event and an
  intervening `request()` call, is delivered **intact and exactly once**.
- **Per-post re-verification** (`signal_chat_it`): a member removed/altered
  mid-body stops the remaining chunks and logs the `WITHHOLDING` notice.
- **Integration** (`signal_chat_it`): end-to-end chat behavior under the
  `signal` feature.
- **F2 pre-emption** (`amplihack-signal` `chat_it`, `turn` module): a
  long-blocking child pre-empted mid-turn resolves to an
  `io::ErrorKind::Interrupted` error; a normal turn still returns its captured
  stdout; the shared `PreemptSlot` is cleared on completion.
- **F7 auto-accept** (`amplihack-signal` transport test against the fake
  endpoint): `create_group` issues a `sendMessageRequestResponse` accept for the
  newly created group id (asserted via the fake's `accepted_groups()`); a group
  is not returned until its message request has been accepted.
- **F8 broadened redaction** (`amplihack-signal` `chat_it`, `outbound` module):
  short values (`token=x`, `password: abc`) and punctuation-heavy values
  (`api_key: a!b#c$d%e^f&g`, `secret = example!#%notreal`, quoted `password="example!%"`)
  are redacted and the raw value does not survive; `redact_for_relay` applied
  twice equals once for a body mixing several secret shapes (idempotency);
  keyword-free prose is returned verbatim; and over-redaction is bounded — the
  ` required` tail in `password: is required` is preserved. The two pre-existing
  outbound tests (`redact_for_relay_removes_bearer_secret`,
  `redaction_happens_before_chunking_so_no_chunk_leaks_a_secret`) continue to
  pass.

---

## See also

- [Signal External Service Integration](signal-external-integration.md)
- [Signal Channel](signal-channel.md)
- [Signal Onboarding](SIGNAL_ONBOARDING.md)
