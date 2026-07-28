# Relay Secret-Redaction Hardening

> [Home](../index.md) > [Security](./README.md) > Relay Redaction Hardening
>
> Status: Implemented · Severity: P1 (information disclosure) · Crates: `amplihack-redact` (new), `amplihack-signal`, `amplihack-turn`, `amplihack-cli`
> Resolves: [#1096](https://github.com/rysweet/amplihack-rs/issues/1096), [#1103](https://github.com/rysweet/amplihack-rs/issues/1103), [#1108](https://github.com/rysweet/amplihack-rs/issues/1108)
> Related: builds on the bounded turn-failure tail from [#1092](https://github.com/rysweet/amplihack-rs/issues/1092) / PR #1107.

## Summary

The Signal relay captures an accepted agent turn **verbatim** from the
`copilot` child process and forwards it to a group that may contain more than
one member. Any credential a turn pastes, echoes, or fails with can therefore
leave the machine. Redaction is the defense-in-depth control that scrubs those
secrets before any byte is emitted.

This hardening does three things:

1. **Broadens coverage** ([#1096](https://github.com/rysweet/amplihack-rs/issues/1096), [#1103](https://github.com/rysweet/amplihack-rs/issues/1103)) — the relay redactor now catches **Azure DevOps Personal Access Tokens** and **short / unusual-charset** secret tokens it previously missed, and **strengthens** the already-partial **`Bearer` / `Authorization`** coverage into a guaranteed header-value rule. All changes are strictly additive — nothing that was redacted before stops being redacted.
2. **Closes the error-tail leak** ([#1108](https://github.com/rysweet/amplihack-rs/issues/1108)) — the bounded turn-failure error tail is routed through the redactor before it reaches **every** emit boundary (relay body, stderr, and `DEBUG` trace field), not just the relay body.
3. **Centralizes the redactor** — `redact_for_relay` now lives in a new leaf crate, [`amplihack-redact`](#the-amplihack-redact-crate), so both `amplihack-signal` and `amplihack-turn` can call it without a dependency cycle.

Redaction is **additive and strictly widening**: every shape redacted before is
still redacted, benign prose is preserved, and the public API is unchanged.

> **Redaction is defense-in-depth, not the primary control.** Fail-closed
> Signal group-membership verification (see
> [Signal Chat Hardening](../signal-chat-hardening.md), F3/F5/F6) remains the
> primary gate on who may receive a relay. Redaction reduces blast radius if a
> secret is present in an authorized turn; it never replaces authorization.

---

## Contents

- [What gets redacted](#what-gets-redacted)
- [Where redaction is applied (emit boundaries)](#where-redaction-is-applied-emit-boundaries)
- [The `amplihack-redact` crate](#the-amplihack-redact-crate)
  - [`redact_for_relay`](#redact_for_relay)
  - [Properties](#properties)
- [Shell-side redaction: `sanitize_cli_output`](#shell-side-redaction-sanitize_cli_output)
- [Configuration](#configuration)
- [Examples](#examples)
- [Testing](#testing)
- [FAQ](#faq)

---

## What gets redacted

Patterns are applied **in order**. Broader `name = value` assignments are
redacted first so their placeholder cannot re-expose a value a later, narrower
pattern would leave intact. Each pattern replaces the secret with a
**shape-labeled placeholder** that never encodes the secret's length or bytes.

| # | Secret shape | Example (input → output) | Placeholder | Status |
| --- | --- | --- | --- | --- |
| 1 | PEM private-key block | `-----BEGIN … PRIVATE KEY----- … -----END … PRIVATE KEY-----` → `[REDACTED-PRIVATE-KEY]` | `[REDACTED-PRIVATE-KEY]` | existing |
| 2 | Signal device-link URI | `sgnl://linkdevice?uuid=…` → `[REDACTED-SIGNAL-LINK]` | `[REDACTED-SIGNAL-LINK]` | existing |
| 3 | `name: value` / `name = value` credential assignment | `api_key="hunter2abc"` → `api_key=[REDACTED]` | `$1=[REDACTED]` | existing |
| 4 | GitHub tokens (`ghp`/`gho`/`ghu`/`ghs`/`ghr`/`github_pat`) | `ghp_0123…` → `[REDACTED-GITHUB-TOKEN]` | `[REDACTED-GITHUB-TOKEN]` | existing |
| 5 | AWS access-key ID | `AKIA…` → `[REDACTED-AWS-KEY]` | `[REDACTED-AWS-KEY]` | existing |
| 6 | Google API key | `AIza…` → `[REDACTED-GOOGLE-KEY]` | `[REDACTED-GOOGLE-KEY]` | existing |
| 7 | URL userinfo password | `https://user:pass@host` → `https://user:[REDACTED]@host` | `$1:[REDACTED]@` | existing |
| 8 | Slack tokens (`xoxb`/`xoxa`/`xoxp`/`xoxr`/`xoxs`) | `xoxb-…` → `[REDACTED-SLACK-TOKEN]` | `[REDACTED-SLACK-TOKEN]` | existing |
| 9 | HTTP `Bearer` credential | `Bearer eyJ…` → `[REDACTED-BEARER]` | `[REDACTED-BEARER]` | existing |
| 10 | **Azure DevOps PAT** | `AZURE_DEVOPS_PAT=abcdef…` and 52-char base32 PAT bodies in `Authorization`/`:pat@` context → `[REDACTED-AZDO-PAT]` | `[REDACTED-AZDO-PAT]` | **new (#1103)** |
| 11 | **`Authorization:` header value** | `Authorization: Basic dXNlcjpwYXNz` → `Authorization: [REDACTED]` | `Authorization: [REDACTED]` | **strengthened (#1103)** |
| 12 | **Short / unusual-charset token** (quote- or scheme-gated) | `token: 'a1$x'` → `token=[REDACTED]` | `$1=[REDACTED]` | **new (#1096)** |

### New coverage detail

- **Azure DevOps PATs (#1103).** AzDO PATs have no fixed vendor prefix, so they
  are matched by **context** — a base32/base64 body of the expected length that
  appears in an `Authorization` header, a `:<pat>@` URL userinfo position, or an
  `AZURE_DEVOPS_*` / `*_PAT` assignment. This avoids redacting arbitrary
  base64-looking prose while catching the real credential shape.
- **`Bearer` / `Authorization` (#1103).** `Authorization:` header values are
  **already partially covered** today: the pre-existing credential-assignment
  rule (pattern #3) lists `authorization` in its keyword alternation, and its
  value class `[A-Za-z0-9._~+/=:-]{6,}` already matches shapes such as
  `Authorization: Basic <base64>`. #1103 does not introduce net-new coverage
  here so much as it **strengthens and guarantees** that coverage with a
  dedicated `Authorization:` header-value rule: it redacts the value regardless
  of scheme (`Basic`, `Bearer`, custom) while keeping the header *name* intact,
  so log lines stay diagnostic and the guarantee no longer depends on the
  generic assignment rule's value-class incidentally matching. This keeps the
  overall change **strictly additive** — no previously redacted string stops
  being redacted.
- **Short / unusual-charset tokens (#1096).** The assignment rule (pattern #3)
  is **widened, never loosened**: short values and values containing unusual
  characters are redacted **only** when they appear quoted or after an auth
  scheme, so ordinary prose such as `secret sauce` or `password reset` is
  preserved. The pre-existing `{6,}` general assignment behavior is unchanged;
  the new coverage is an additional, gated alternation.

> **Backward compatibility:** coverage only ever **widens**. Every assertion in
> the pre-existing redaction test suite (`crates/amplihack-signal/tests/chat_it.rs`)
> still passes unchanged. See [Testing](#testing).

---

## Where redaction is applied (emit boundaries)

The turn-failure error tail (introduced bounded by [#1092](https://github.com/rysweet/amplihack-rs/issues/1092) / PR #1107)
reaches operators through **four** sinks. Every sink now redacts **before** it
emits:

| Sink | Location | Audience | How it is redacted |
| --- | --- | --- | --- |
| Relay body | `amplihack-cli` `signal/chat.rs` (relay `TurnOutput`) | Signal group members | `redact_and_chunk` → `redact_for_relay` over the whole body before chunking |
| stderr | `amplihack-cli` `signal/chat.rs` session-loop error line (`eprintln!` at chat.rs:241) | Local logs / operator console | the whole formatted error value (`e.to_string()`) passed through `redact_for_relay` inside the existing `eprintln!` |
| `DEBUG` trace field | `amplihack-turn` `turn.rs` `tracing::debug!(output = …)` | Trace/OTel sinks (**wider** than the relay) | field value passed through `redact_for_relay` before the event is recorded |
| Turn `io::Error` string | `amplihack-turn` `turn.rs` | Callers that format the error | bounded tail passed through `redact_for_relay` before it is embedded in the error string |

**Redact-before-embed rule.** Redaction is always applied to the tail *before*
it is concatenated into an `io::Error` or captured by a `tracing` field —
formatting a raw value first would defeat redaction. The `io::Error` surface
and the [#1092](https://github.com/rysweet/amplihack-rs/issues/1092) / PR #1107
truncation logic are **not** changed; only a redaction pass is inserted ahead of
them.

```text
       copilot child stdout/stderr (verbatim, may contain secrets)
                              │
                     bounded tail (#1092 / #1107)
                              │
                    redact_for_relay(tail)     ◄── single choke point
                              │
        ┌──────────────┬──────────────┬───────────────────┐
        ▼              ▼              ▼                   ▼
   relay body      stderr line    DEBUG field       io::Error string
   (Signal)        (local log)    (trace/OTel)      (caller-formatted)
```

> **Note on the stderr sink.** `redact_for_relay` is the *single function* used
> at every boundary, but the inputs differ. The relay body, `DEBUG` field, and
> `io::Error` surfaces operate on the per-turn **bounded tail** shown above. The
> stderr sink is the session-loop `eprintln!` at `chat.rs:241`, which formats the
> **whole error value** `e`; #1108 redacts that value in place —
> `eprintln!("…: {}", redact_for_relay(&e.to_string()))` — rather than a bounded
> tail. The sink type is unchanged (it stays an `eprintln!`); only its payload is
> redacted.

---

## The `amplihack-redact` crate

`redact_for_relay` previously lived in `amplihack-signal::chat::outbound`.
Because `amplihack-turn` must call it (to close the [#1108](https://github.com/rysweet/amplihack-rs/issues/1108)
error-tail leak) and the dependency direction is `signal → turn`, the redactor
was extracted into a new **leaf crate** to avoid a dependency cycle.

- **Crate:** `amplihack-redact`
- **Dependencies:** `regex` only (already a workspace dependency; adds no
  `tokio`, networking, or process deps to `amplihack-turn`).
- **Location:** `crates/amplihack-redact/`

`amplihack-signal::chat::outbound` retains the same public path via a
re-export, so **every existing caller and test is unaffected**:

```rust
// crates/amplihack-signal/src/chat/outbound.rs
pub use amplihack_redact::redact_for_relay;
```

### `redact_for_relay`

```rust
/// Scrub high-frequency secret shapes out of `body`.
///
/// Pure, deterministic, idempotent, and allocation-light (only adopts a new
/// buffer on a real match). Safe to call at any emit boundary.
#[must_use]
pub fn redact_for_relay(body: &str) -> String;
```

| Aspect | Guarantee |
| --- | --- |
| Purity | No I/O, no side effects, no global mutable state. |
| Determinism | Same input → same output, on every platform. |
| Idempotency | `redact_for_relay(redact_for_relay(x)) == redact_for_relay(x)`. |
| Monotonicity | Coverage strictly widens vs. the prior version — never narrows. |
| Failure mode | Fail-closed: on any internal error it returns a redacted-or-empty string, never the raw input. |
| Panics | None. UTF-8-boundary-safe; never panics on multibyte input or on an attacker-influenced tail. |

`redact_and_chunk` is unchanged and still the correct entry point for the relay
path — it redacts over the **whole** body first, then chunks, so a secret can
never straddle (and survive in) a chunk boundary:

```rust
#[must_use]
pub fn redact_and_chunk(body: &str) -> Vec<String>; // = chunk(&redact_for_relay(body))
```

### Properties

- **ReDoS-safe (CWE-1333).** All patterns are bounded/anchored with no nested
  quantifiers or catastrophic backtracking; runtime is linear and is asserted by
  an adversarial-length test.
- **Length-oblivious placeholders.** Placeholders (`[REDACTED-…]`) encode
  neither the secret's length nor any of its bytes.
- **No persistence.** Pre-redaction values are never cached, logged, or written
  to disk.
- **No stray output.** New Rust code emits nothing via `print!` / `println!` /
  `eprintln!`; observability goes through `tracing` + OTel only. (Pre-existing
  `eprintln!` sinks are left in place, but their payload is now redacted.)

---

## Shell-side redaction: `sanitize_cli_output`

The workflow-prep recipe scrubs captured CLI output in shell before it is
surfaced. Two `sanitize_cli_output` `sed` chains exist and are kept **in sync**:

- `amplifier-bundle/recipes/workflow-prep.yaml` — first chain (~line 267)
- `amplifier-bundle/recipes/workflow-prep.yaml` — second chain (~line 393)

Both chains gained, after the existing basic-auth / GitHub-token rules and in
identical order:

- an **Azure DevOps PAT / long base32-base64** catch-all, and
- a generic **`Bearer` / `Authorization`** value redaction.

Editing only one chain is a defect. The reliability test
`amplifier-bundle/recipes/tests/test-issue-1103-relay-redaction.sh` asserts
that **both** chains redact an AzDO PAT and a `Bearer` token.

---

## Configuration

Redaction is **always on** for the relay and error-tail paths — there is no
opt-out, and the pattern set is not user-configurable. This is intentional:
redaction is a security control, and a toggle would be a foot-gun.

Related knobs that affect *how much* is emitted (not *whether* it is redacted):

| Setting | Effect | Default |
| --- | --- | --- |
| Turn-failure tail bound (#1092 / #1107) | Caps how many bytes of child output are surfaced on failure. Redaction runs on whatever bytes remain. | bounded tail (see PR #1107) |
| Full-output DEBUG gating (#1092 / #1107) | Full child output is only recorded at `DEBUG`; that field is redacted too. | gated behind `DEBUG` |
| `signal` Cargo feature | The relay path (`chat/` module) is compiled only with the `signal` feature. `amplihack-redact` itself has no feature gate. | feature-gated |

---

## Examples

### Relaying a turn (already wired through redaction)

```rust
use amplihack_signal::chat::outbound::redact_and_chunk;

let body = "deploy done. token: ghp_ABCDEFGHIJKLMNOPQRST1234 and \
            Authorization: Bearer eyJhbGciOi.JIUzI1.sig";
for chunk in redact_and_chunk(body) {
    // "…token: [REDACTED-GITHUB-TOKEN] and Authorization: [REDACTED]"
    transport.post(chunk).await?;
}
```

### Redacting an error tail before emit (#1108)

```rust
use amplihack_redact::redact_for_relay;

// bounded tail from the failed turn (#1092 / #1107)
let tail: &str = bounded_turn_failure_tail;

// stderr sink — the pre-existing `eprintln!` at chat.rs:241 is kept in place;
// only its payload is redacted. The sink emits the *whole* formatted error
// value, so redaction is applied to `e.to_string()`, not to a bounded tail.
eprintln!(
    "signal chat: session loop ended with error: {}",
    redact_for_relay(&e.to_string())
);

// relay sink — defense-in-depth explicit redaction at the boundary
let relay = TurnOutput::from_text(format!("turn failed: {}", redact_for_relay(tail)));
```

### New coverage, before vs. after

```text
Azure DevOps PAT (#1103)
  in : "clone https://user:abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnop2345@dev.azure.com/org"
  out: "clone https://user:[REDACTED-AZDO-PAT]@dev.azure.com/org"

Authorization header (#1103)
  in : "Authorization: Basic dXNlcjpwYXNzd29yZA=="
  out: "Authorization: [REDACTED]"

Short / unusual-charset token (#1096)
  in : "token: 'a1$x'"      out: "token=[REDACTED]"
  in : "the password reset link"   out: "the password reset link"   (benign prose preserved)
```

---

## Testing

| Layer | File | Covers |
| --- | --- | --- |
| Unit (crate) | `crates/amplihack-redact/src/lib.rs` (`#[cfg(test)]`) | Each new pattern (AzDO PAT, `Authorization`, short token, unusual charset); benign-prose negatives; idempotency; monotonic coverage; ReDoS bound; multibyte boundary; fail-closed. |
| Characterization | `crates/amplihack-signal/tests/chat_it.rs` | All pre-existing assertions stay green; re-export smoke test. |
| Regression (#1108) | `crates/amplihack-turn/tests/turn_error_redaction_it.rs` | Inject a secret into a failing turn's stdout/stderr; assert placeholder-only in the returned `io::Error` string, and that the error shape is preserved for benign output. |
| Shell (#1103) | `amplifier-bundle/recipes/tests/test-issue-1103-relay-redaction.sh` | Both `sanitize_cli_output` chains are byte-identical and redact an AzDO PAT and a `Bearer` token; existing GitHub-token / URL-userinfo assertions still pass. |

**Security acceptance gates (all must pass):**

1. Per-secret-class injection (AzDO PAT, `Bearer`/`Authorization`, short-token,
   unusual-charset) yields **placeholder-only** across the relay, stderr, and
   `DEBUG` sinks.
2. Idempotency and monotonic-coverage hold.
3. ReDoS and multibyte-boundary tests pass.
4. Both shell `sed` chains are covered.
5. A forced-error path is fail-closed (never emits the raw tail).

Run locally:

```bash
cargo test -p amplihack-redact
cargo test -p amplihack-signal --features signal
cargo test -p amplihack-turn
bash amplifier-bundle/recipes/tests/test-issue-1103-relay-redaction.sh
```

---

## FAQ

**Does this change the public API?** No. `redact_for_relay` and
`redact_and_chunk` keep the same signatures and the same
`amplihack_signal::chat::outbound` import path (now a re-export from
`amplihack-redact`).

**Could the wider #1096 rule redact benign text?** The new short /
unusual-charset coverage is **gated** on quotes or an auth scheme, and negative
tests assert that ordinary prose (`password reset`, `secret sauce`) is left
intact. The pre-existing general `{6,}` assignment rule is unchanged.

**Why a new crate instead of putting it in `amplihack-utils`?** `amplihack-turn`
is intentionally minimal; depending on `amplihack-utils` would pull in
`tokio`/process/network deps. The `amplihack-redact` leaf crate adds only
`regex`. (If a new crate is rejected in review, the fallback is a
`default-features = false`, `regex`-only `redact` module in `amplihack-utils`.)

**Is redaction enough on its own?** No — it is defense-in-depth. Group-membership
verification (see [Signal Chat Hardening](../signal-chat-hardening.md)) is the
primary authorization control and its fail-closed ordering is unchanged.

---

## See also

- [Signal Chat Hardening](../signal-chat-hardening.md) — membership verification and the relay path
- [Signal Chat](../SIGNAL_CHAT.md) — user-facing Signal chat overview
- [Azlin stderr Redaction](./AZLIN_STDERR_REDACTION.md) — the sibling redactor for Azure CLI stderr
- [Token Sanitization Guide](./TOKEN_SANITIZATION_GUIDE.md) — log-sink token sanitization
- [Security API Reference](./SECURITY_API_REFERENCE.md)
- [Security Testing Guide](./SECURITY_TESTING_GUIDE.md)
