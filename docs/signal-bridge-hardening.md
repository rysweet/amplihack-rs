# Signal Bridge Hardening (Phase A)

Hardening reference for the Phase A Signal bridge that spans
[`crates/amplihack-signal`](../crates/amplihack-signal) and
[`crates/amplihack-cli`](../crates/amplihack-cli). It documents three
review-feedback fixes — a consolidated loopback endpoint validator (**F1**), a
fail-closed group-membership parser (**F3**), and a documented child
pre-emption TOCTOU residual (**F2**).

Everything described here is compiled only under the `signal` Cargo feature. The
crate compiles cleanly both feature-on and feature-off; with the feature off,
none of the items below are present.

> **Status — forward specification.** This document describes the *intended*
> hardened state. The items it hardens (the `amplihack-signal` `bridge/` module,
> `transport::parse_group_members`, and the CLI `preempt_child`) land with
> Phase A (issue #1054) and are **not present on a branch cut from `main`**.
> Until this work is rebased onto the Phase A base, treat the present-tense
> descriptions below as the target contract for the F1/F2/F3 changes rather than
> as shipped behavior. The single item that already exists today is the CLI
> `signal::validate::validate_loopback_endpoint`, which F1 reduces to a delegate.

Read this document when you need to:

- validate a remote relay endpoint and understand which hosts/ports are
  accepted,
- reason about why a Signal group membership is classified `Unverified` and the
  relay withheld,
- audit the known PID-reuse pre-emption window and its mitigation.

---

## Contents

- [Overview](#overview)
- [F1 — Canonical loopback endpoint validator](#f1--canonical-loopback-endpoint-validator)
  - [`validate_loopback_endpoint`](#validate_loopback_endpoint)
  - [Acceptance / rejection matrix](#acceptance--rejection-matrix)
  - [`bridge::validate_endpoint` delegation](#bridgevalidate_endpoint-delegation)
  - [CLI delegation](#cli-delegation)
- [F3 — Fail-closed membership parse](#f3--fail-closed-membership-parse)
- [F2 — Child pre-emption PID-reuse TOCTOU](#f2--child-pre-emption-pid-reuse-toctou)
- [Security invariants](#security-invariants)
- [Exit-code taxonomy](#exit-code-taxonomy)
- [Testing](#testing)
- [See also](#see-also)

---

## Overview

The Signal bridge relays messages between a Signal group and a local
Copilot/agent runtime. Two trust boundaries are hardened here:

1. **Network egress** — the bridge will only dial a **loopback** relay endpoint
   unless an operator explicitly opts into an unsafe remote. Prior to this pass
   two validators disagreed on what "loopback" meant; F1 makes a single
   canonical validator the source of truth.
2. **Authorization** — a message is only relayed to a Signal group whose
   membership can be positively verified against E.164 numbers. F3 ensures a
   member with a missing/unparseable number can never be silently dropped from
   that check.

Both boundaries are **fail-closed**: any ambiguous, unparseable, or missing
input results in rejection (endpoint) or `Unverified` classification
(membership), never a default-allow.

---

## F1 — Canonical loopback endpoint validator

Before this pass there were **two** divergent validators:

| Location | Behavior |
| --- | --- |
| `amplihack_signal::bridge::validate_endpoint` (runtime) | bespoke host/port split; **false-rejected** the bare IPv6 loopback `::1:9000` |
| `amplihack_cli` `signal::validate::validate_loopback_endpoint` (CLI) | correct `rsplit_once(':')` split; already **accepts** bare `::1:9000`, rejects port `0`/out-of-range/wildcard host |

They are now consolidated into a **single canonical, lexical-only** validator
that lives in the signal crate (dependency direction is CLI → signal, so the
canonical implementation is hoisted *down* into the signal crate). The canonical
validator adopts the CLI's existing "last-colon-wins" semantics verbatim; the
runtime and the CLI both delegate to it. Net validator LOC drops.

The only intended behavior change is on the **runtime `bridge::validate_endpoint`
path**, which previously false-rejected the bare, bracket-less IPv6 loopback
`::1:9000` and now **ACCEPTS** it (matching the CLI). The **CLI path is fully
behavior-preserving** — it already accepted bare `::1` and rejected zero/wildcard
ports, so the consolidation only removes its now-duplicate `split_host_port`
helper without changing any accept/reject outcome. Everything else on both paths
is unchanged.

### `validate_loopback_endpoint`

```rust
// crate: amplihack-signal  (feature = "signal")
use amplihack_signal::bridge::{validate_loopback_endpoint, EndpointError};

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

### `bridge::validate_endpoint` delegation

The runtime entry point keeps its unsafe-remote short-circuit and then delegates:

```rust
// crate: amplihack-signal  (feature = "signal")
pub fn validate_endpoint(endpoint: &str, unsafe_remote: bool)
    -> Result<(), BridgeError>
{
    // Explicit operator opt-in bypasses loopback enforcement.
    if unsafe_remote {
        return Ok(());
    }
    // Single source of truth; any failure maps to a rejection.
    validate_loopback_endpoint(endpoint)
        .map_err(|_| BridgeError::RemoteEndpointRejected)
}
```

- `unsafe_remote = true` remains the **only** non-loopback path. With it set,
  routable endpoints such as `10.0.0.5:8080` are accepted.
- All rejections surface as `BridgeError::RemoteEndpointRejected` (exit code
  `2` — see [Exit-code taxonomy](#exit-code-taxonomy)). No new `BridgeError`
  variants and no new success paths were added.
- The previous bespoke `is_loopback_host` helper and inline host/port splitting
  are deleted.

### CLI delegation

The CLI keeps its public function name and signature so callers are untouched;
it becomes a thin `anyhow`-wrapping delegate to the canonical validator:

```rust
// crate: amplihack-cli  (feature = "signal")
pub fn validate_loopback_endpoint(endpoint: &str) -> anyhow::Result<()> {
    amplihack_signal::bridge::validate_loopback_endpoint(endpoint)
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
only the `Json` variant; the Phase A base adds the frame/transport variants that
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

## F2 — Child pre-emption PID-reuse TOCTOU

`preempt_child` (CLI signal bridge) terminates an in-flight Copilot turn by
sending a signal to a PID stored in a shared `Arc<Mutex<Option<u32>>>` slot.
Because it operates on a **raw PID** rather than an owned `Child` handle, there
is a time-of-check/time-of-use window: between reading the slot and issuing the
kill, the original process could have exited and the OS could have recycled the
PID, so the signal could in principle be delivered to an unrelated process.

**Decision for this hardening pass:** the full fix (storing and `.kill()`-ing an
owned `Child`) requires restructuring runner ownership — the runner spawns and
immediately consumes the `Child` via `wait_with_output()` in one task while
`preempt_child` fires from a different task — and is out of scope. Instead the
residual is **explicitly documented** in code:

- `preempt_child` carries a doc-comment describing the PID-reuse / PID-wraparound
  window.
- The comment references the existing mitigation: the runner **clears the shared
  slot to `None` on turn exit**, which closes the window in the common case
  (the slot no longer holds a stale PID once the child has been reaped).

No behavior change is introduced by F2; it makes the accepted residual
auditable rather than hidden.

---

## Security invariants

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
- **PII discipline.** Neither `EndpointError` nor `WireError` `Display` embeds a
  resolved address or phone number — they reference the defect, not the value.

---

## Exit-code taxonomy

| Condition | Error | Exit code |
| --- | --- | --- |
| Endpoint rejected (non-loopback, bad port, DNS name, missing port) | `BridgeError::RemoteEndpointRejected` | `2` |

F1 preserves the existing taxonomy: every endpoint rejection continues to map to
`RemoteEndpointRejected` / exit `2`. No new codes were introduced.

---

## Testing

Validation gate for the hardening pass (all `signal`-feature-gated):

```bash
cargo fmt --all
cargo clippy -p amplihack-signal --features signal --all-targets -- -D warnings
cargo test  -p amplihack-signal --features signal
cargo test  -p amplihack-cli --test signal_bridge_it --test signal_validator_parity
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
- **Integration** (`signal_bridge_it`): end-to-end bridge behavior under the
  `signal` feature.

---

## See also

- [Signal External Service Integration](signal-external-integration.md)
- [Signal Channel](signal-channel.md)
- [Signal Onboarding](SIGNAL_ONBOARDING.md)
