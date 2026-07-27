# Turn-Driver Characterization Tests (Issue #910, PR-1)

A safety net that locks the **current observable behavior** of the two
turn-driver loops before a later, multi-PR refactor extracts a shared
`Session`/`Channel` abstraction. These are **characterization tests only** — they
describe existing behavior, they do not change production code, and every later
PR must keep them green **unchanged**.

- **Signal loop**: `crates/amplihack-cli/src/commands/signal/chat.rs`
  (`run_chat_async`), driven by `SerialTurnDriver` + `CopilotTurnRunner` from
  `crates/amplihack-signal/src/chat/turn.rs`.
- **auto-mode loop**: `crates/amplihack-launcher/src/auto_mode_exec.rs`
  (`AutoMode::run`, `should_continue`, `build_execution_prompt`).

> These tests are a *retcon* specification: they assert what the code already
> does today. If a change makes one fail, the change altered observable
> behavior — that is the signal the safety net exists to raise.

---

## The seven locked invariants

| ID    | Invariant | Locked by | Status |
| ----- | --------- | --------- | ------ |
| INV-1 | **Turn serialization** — at most one turn per session runs at a time (`SerialTurnDriver`). | `chat_it.rs::turn::turns_execute_one_at_a_time_per_session` | pre-existing (cited) |
| INV-2 | **Post-on-completion** — a completed turn's output is posted outward exactly once, only on verified operator-only membership; withheld when membership is unverified. | `verified_send_it.rs`, `session_relay_it.rs` (outbound), `chat_membership_failclosed_it.rs` | pre-existing (cited) |
| INV-3 | **Inbound gating (fail-closed)** — an inbound message is accepted as a next-turn prompt only if `Gate::evaluate` accepts it (wrong sender/device/group/echo/empty ⇒ rejected). | `gating.rs` unit tests + `session_relay_it.rs`; consolidated end-to-end anchor **added**: `chat_it.rs::gating::characterization_inv3_inbound_gate_failclosed` | pre-existing + **added anchor** |
| INV-4 | **Bounded turn queue** — when operator-configurable capacity is exceeded, the **oldest** pending prompt is evicted (backpressure), matching `Inbox` semantics. | `session_channel_it.rs::characterization_inv4_capacity_from_operator_config_evicts_oldest` | **added** |
| INV-5 | **Resume continuity** — each turn resumes the **same** session id (`build_turn_argv`), preserving context; the prompt is exactly one argv element (injection-safe), verified across **two successive** turns. | `chat_it.rs::turn::characterization_inv5_successive_turns_reuse_same_session_id` | **added** |
| INV-6 | **No per-turn wall-clock timeout** — a turn runs to natural completion; the loop injects no wall-clock cap. | `chat_it.rs::turn::characterization_inv6_turn_runs_to_completion_no_wallclock_cap` | **added** |
| INV-7 | **auto-mode continuation** — continuation prompts are produced each turn until the loop's stop condition, and the stop condition is honored. | `auto_mode_exec.rs::tests::characterization_inv7_automode_continues_until_stop_condition` | **added** |

All seven invariants are covered: INV-1, INV-2 by citation only; INV-3 by
citation **plus** a new consolidated anchor; INV-4, INV-5, INV-6, INV-7 by new
characterization tests.

---

## How to run

The Signal-side tests are gated on the `signal` cargo feature (matching every
existing Signal test file, which begins with `#![cfg(feature = "signal")]`). A
default, feature-off build compiles them away to nothing.

```bash
# Signal turn-driver + inbox invariants (INV-1..INV-6)
cargo test -p amplihack-signal --features signal

# Narrower: a single characterization test file
cargo test -p amplihack-signal --features signal --test chat_it
cargo test -p amplihack-signal --features signal --test session_channel_it

# CLI-side signal loop coverage (INV-2 membership / outbound)
cargo test -p amplihack-cli --features signal

# auto-mode continuation loop (INV-7) — runs in the DEFAULT build, no feature flag
cargo test -p amplihack-launcher auto_mode_exec
```

### The four cargo gates

This is a Rust change and follows the default workflow; all four gates pass with
and without `--features signal` as appropriate:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features signal -- -D warnings
cargo build --workspace --features signal
cargo test  --workspace --features signal
```

---

## Test seams (reused, never re-invented)

These tests are deterministic and hermetic: **no** real network, **no** real
`copilot`/`signal-cli` process, and **no** `sleep`-as-synchronization. They ride
the seams that already exist in the crates.

### `fake_endpoint` — in-process signal-cli loopback

`crates/amplihack-signal/src/fake_endpoint.rs` is an in-process loopback fake of
the `signal-cli` JSON-RPC daemon. Tests script inbound envelopes and observe
outbound sends without touching a socket. Used by the INV-3 anchor to drive
accepted vs. rejected inbound messages end-to-end.

### `TurnRunner` — injectable turn executor (`turn.rs`)

```rust
pub trait TurnRunner: Send + Sync {
    fn run_argv(&self, argv: Vec<String>)
        -> Pin<Box<dyn Future<Output = io::Result<String>> + Send>>;
}
```

`CopilotTurnRunner` is the production implementation (spawns real `copilot`).
Characterization tests inject a **mock** instead:

- **Recording mock** (INV-5): captures the exact `argv` of every `run_turn`
  call so the test can assert `--session-id <same-uuid>` on turn 1 and turn 2,
  and that the prompt occupies exactly one argv slot (verbatim, never
  shell-concatenated).
- **Channel-gated mock** (INV-6): blocks inside `run_argv` on a test-controlled
  `oneshot`/`Notify` until the test releases it. The test asserts the turn is
  *not* complete before release and completes *after* release — proving the loop
  imposes no wall-clock cap (fully deterministic, no timing races).

`SerialTurnDriver::run_turn` holds an async mutex across the whole child
lifetime, which is what INV-1 (pre-existing) and INV-5 (successive turns) rely on.

### `Inbox` — bounded, operator-configurable queue (`session_channel.rs`)

INV-4 drives the **operator-config** path rather than an explicit `new(_, N)`:

```rust
// Operators tune capacity via the environment; invalid/zero/absent → DEFAULT_CAPACITY (32).
pub fn default_capacity() -> usize; // reads AMPLIHACK_SIGNAL_INBOX_CAPACITY
```

The test sets `AMPLIHACK_SIGNAL_INBOX_CAPACITY`, constructs the inbox through
`default_capacity()`, pushes `capacity + 1` prompts, and asserts:

- the final push returns `PushOutcome::EvictedOldest`, and
- the surviving queue is the newest `capacity` prompts in FIFO order (the
  **oldest** was evicted).

Environment mutation is serialized with a save/restore guard and a unique
capacity value so the test cannot contaminate sibling tests.

### `AutoModeRunner` mock seam — auto-mode loop (`auto_mode_exec.rs`)

INV-7 characterizes the loop **arithmetic** using the real, in-crate decision
functions — no process is spawned:

- `should_continue()` — honors `max_turns`, `max_api_calls`, `max_output_bytes`,
  and `max_session_secs`.
- `build_execution_prompt()` — emits a per-turn `[Turn {turn+1}/{max_turns}]`
  marker (1-based; the internal `turn` index is 0-based).

With `max_turns = 3`, the test replays the loop's decision sequence and asserts a
continuation prompt is produced for turns 1..=3, each carrying the correct
`[Turn {turn+1}/{max_turns}]` marker, and that `should_continue()` returns `false` once
`turn >= max_turns` (the stop condition is honored). This runs in the default
build; it does **not** require the `signal` feature.

---

## Naming convention

Every added test is named after the invariant it locks:

```
characterization_inv3_inbound_gate_failclosed
characterization_inv4_capacity_from_operator_config_evicts_oldest
characterization_inv5_successive_turns_reuse_same_session_id
characterization_inv6_turn_runs_to_completion_no_wallclock_cap
characterization_inv7_automode_continues_until_stop_condition
```

The `characterization_inv<N>_` prefix makes it trivial to grep the whole safety
net and to spot, in a later refactor PR's diff, exactly which invariant a change
touched.

---

## File placement (no new `[[test]]` entries)

Added tests live inside **already-registered** test files/modules so the refactor
does not introduce Cargo `[[test]]` drift:

| Invariant | File | Module / location |
| --------- | ---- | ----------------- |
| INV-3 anchor | `crates/amplihack-signal/tests/chat_it.rs` | `mod gating` |
| INV-5, INV-6 | `crates/amplihack-signal/tests/chat_it.rs` | `mod turn` |
| INV-4 | `crates/amplihack-signal/tests/session_channel_it.rs` | file scope |
| INV-7 | `crates/amplihack-launcher/src/auto_mode_exec.rs` | `#[cfg(test)] mod tests` |

---

## Guarantees (what these tests promise the refactor)

1. **Tests only.** `git diff --stat` for this PR lists only the three files
   above (plus any `cfg(test)`-gated test-support helpers). No production logic —
   including `gating.rs` and every turn-driver path — is modified or weakened.
2. **Deny-by-default is asserted positively.** Each INV-3 rejection reason (wrong
   sender, wrong group, wrong device, empty body, echo-within-TTL) gets an
   explicit negative assertion, so a future *fail-open* regression breaks a test
   rather than silently passing.
3. **Injection safety is a security control.** INV-5 asserts the prompt is a
   single argv element on **both** turns; a future change that shell-concatenates
   the prompt fails the test.
4. **Hermetic and deterministic.** Loopback fake + injected mock runner; no
   network egress, no real `signal-cli`/`copilot` spawn, no sleep-based
   synchronization. Env mutation is serialized and restored.
5. **Feature-gate preserved.** Signal-side tests are inert without
   `--features signal`, consistent with the existing test files; auto-mode tests
   pass in the default build.

---

## For the next PR in the series

When you extract the shared `Session`/`Channel` abstraction:

- **Do not edit these tests.** They must pass unchanged. If one fails, you
  changed observable behavior — reconcile the change or the invariant, don't
  relax the test.
- Keep the same seams (`fake_endpoint`, `TurnRunner`, `Inbox`,
  `AutoMode::should_continue`/`build_execution_prompt`) available to the tests,
  even if their internals move behind the new abstraction.
- If the new abstraction renames a type the tests reference, update the
  **reference**, not the **assertion**.
