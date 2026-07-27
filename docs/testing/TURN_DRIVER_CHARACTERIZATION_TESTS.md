# Turn-Driver Characterization Tests

> **Status:** Landed — PR-1 of the Issue #910 multi-PR refactor.
> **Scope:** Test-only safety net. This suite adds **no** production behavior; it
> locks the *current* observable behavior of the two turn-driver loops so the
> later refactor PRs can move code with a guardrail underneath them.

## What this is

A [characterization test](https://en.wikipedia.org/wiki/Characterization_test)
suite (a.k.a. "golden master" / "pin-down tests") that captures the behavior the
two turn-driver loops exhibit *today*, exactly as-is — bugs, quirks, and all.
Its job is **not** to assert what the code *should* do; it is to fail loudly if a
future refactor changes what the code *currently* does.

The two loops under characterization are:

| Loop | Location | Drivers |
| ---- | -------- | ------- |
| **Signal in-process chat loop** | `crates/amplihack-cli/src/commands/signal/chat.rs` | `SerialTurnDriver` + `CopilotTurnRunner` from `crates/amplihack-signal/src/chat/turn.rs` |
| **Auto-mode loop** | `crates/amplihack-launcher/src/auto_mode_exec.rs` (`AutoMode::run`) | in-crate loop, no external driver |

All tests are hermetic: no real network, no real Signal group, no long-lived
child processes used as timers, and no `sleep`-as-synchronization. Timing is
driven through explicit seams (`Gate::evaluate_at`, `Gate::record_outbound_at`),
ordering through channels/atomics, and cross-process state through `tempfile`
paths. This makes them deterministic and safe to run in CI.

## The seven invariants

| ID | Invariant | Why it matters |
| -- | --------- | -------------- |
| **INV-1** | **Turn serialization** — at most one `copilot --session-id <same>` process runs at a time. | Two concurrent turns on the same session race the same session state and corrupt continuity. |
| **INV-2** | **Post-on-completion only on verified membership** — every outbound post re-checks group membership immediately before sending and withholds on anything other than an exact operator-only match. | Fail-closed prevents leaking work output to a group whose membership drifted. |
| **INV-3** | **Inbound gating fail-closed** — an inbound envelope becomes an instruction only if it passes *every* gate check; empty allowlist denies everything; errors/ambiguity deny. | Prevents command injection and re-ingesting the bot's own echoes. |
| **INV-4** | **Bounded turn queue evicts oldest at operator-configurable capacity** — the inbox holds at most `AMPLIHACK_SIGNAL_INBOX_CAPACITY` (default 32) instructions; overflow evicts the oldest. | Bounded memory / DoS mitigation under a flood of inbound messages. |
| **INV-5** | **Resume continuity** — successive turns reuse the *same* session id, and the prompt is passed as exactly one injection-safe argv element. | Continuity across turns; command-injection boundary. |
| **INV-6** | **No per-turn wall-clock timeout** — a turn runs to natural completion; nothing caps a single turn on elapsed time. | Long legitimate turns must not be killed mid-flight. |
| **INV-7** | **Auto-mode continues until a stop condition** — the loop keeps issuing turns until max-turns / max-api-calls / max-output / max-session-duration is hit. | Autonomous progress; predictable termination. |

> **Scope note on INV-6 (intentional addition).** The design spec enumerates
> INV-1, INV-2, and INV-3/4/5/7 explicitly. **INV-6 (no per-turn wall-clock
> timeout) is an additional invariant introduced by this characterization
> suite, not by the original spec.** It is included deliberately: it is
> code-grounded (`SerialTurnDriver::run_turn` wraps the child process in no
> `tokio::time::timeout` or elapsed-time cap, so a turn runs to natural
> completion) and it protects a real behavior — long legitimate turns must not
> be killed mid-flight. Reviewers should treat INV-6 as intentional scope
> expansion. Note it is distinct from `max_session_secs` (INV-7), which is a
> session-level cap, not a per-turn cap.

## Coverage table (INV → test)

Each invariant is covered either by a **pre-existing** test (cited, not
duplicated) or by a **newly added** characterization test. Added tests follow
the naming convention `characterization_inv<N>_<descriptor>`.

| INV | Coverage | Test(s) | File |
| --- | -------- | ------- | ---- |
| INV-1 | Pre-existing | `turn::turns_execute_one_at_a_time_per_session` (`SerialTurnDriver` + `MockRunner`) | `crates/amplihack-signal/tests/chat_it.rs` |
| INV-2 | Pre-existing | `membership::{exact_expected_set_is_verified_and_may_relay, query_error_is_unverified_and_refuses_relay, unexpected_extra_member_refuses_relay, missing_expected_member_refuses_relay}` | `crates/amplihack-signal/tests/chat_it.rs` |
| INV-2 | Pre-existing | `chat_membership_failclosed_it.rs` (parse/classify fail-closed, no member-number leakage) | `crates/amplihack-signal/tests/chat_membership_failclosed_it.rs` |
| INV-3 | Pre-existing | `gating::tests::{empty_allowlist_denies_everything, rejects_sender_not_on_allowlist, rejects_empty_body, rejects_message_for_other_group, rejects_non_group_message, rejects_non_primary_device_sync_echo, rejects_bot_own_linked_device_sync, rejects_sync_not_authored_by_account, suppresses_*_echo_within_ttl}` | `crates/amplihack-signal/src/gating.rs` |
| **INV-3** | **Added** | `characterization_inv3_inbound_gate_failclosed` (consolidated `fake_endpoint` anchor asserting deny-on-error/ambiguity, never fail-open, via `Gate::evaluate_at`) | `crates/amplihack-signal/tests/chat_it.rs` (`gating` mod) |
| INV-4 | Pre-existing | `session_channel::tests::bounded_capacity_evicts_oldest` (fixed cap); `bounded_inbox_survives_a_flood` | `crates/amplihack-signal/src/session_channel.rs`, `crates/amplihack-signal/tests/session_channel_it.rs` |
| **INV-4** | **Added** | `characterization_inv4_capacity_from_operator_config_evicts_oldest` (exercises the `AMPLIHACK_SIGNAL_INBOX_CAPACITY` env path: valid cap honored; invalid/zero/negative/non-numeric fall back to `DEFAULT_CAPACITY`; `PushOutcome::EvictedOldest` bounds memory) | `crates/amplihack-signal/tests/session_channel_it.rs` |
| **INV-5** | **Added** | `characterization_inv5_successive_turns_reuse_same_session_id` (successive `SerialTurnDriver::run_turn` calls emit the same `--session-id`; prompt is exactly one `-p` argv element even with shell metacharacters) | `crates/amplihack-signal/tests/chat_it.rs` (`turn` mod) |
| **INV-6** | **Added** | `characterization_inv6_turn_runs_to_completion_no_wallclock_cap` (a turn returns its output on natural completion with no per-turn elapsed-time cap in the driver) | `crates/amplihack-signal/tests/chat_it.rs` (`turn` mod) |
| INV-7 | Pre-existing | `should_continue_respects_turns`, `should_continue_respects_api_calls` | `crates/amplihack-launcher/src/auto_mode_exec.rs` (`#[cfg(test)] mod tests`) |
| **INV-7** | **Added** | `characterization_inv7_automode_continues_until_stop_condition` (loop continues across turns and terminates on the first tripped stop condition; `build_execution_prompt` emits 1-based `[Turn n/max]`) | `crates/amplihack-launcher/src/auto_mode_exec.rs` (`#[cfg(test)] mod tests`) |

## The added tests

### `characterization_inv3_inbound_gate_failclosed`

Consolidated fail-closed anchor for the inbound gate. Uses the deterministic
`Gate::evaluate_at(envelope, now)` seam (fixed `Instant`s) so no wall-clock time
enters the assertion. Locks that the gate **denies** on every non-happy path
(wrong group, empty body, non-allowlisted sender, empty allowlist, sync from a
non-primary device, sync authored by a non-account number, and a recently-sent
outbound echo inside the TTL) and **only** accepts an exact, well-formed
operator instruction. The point is to pin the *deny-by-default* posture: any
future change that turns one of these denials into an accept must break this
test.

Placement: `crates/amplihack-signal/tests/chat_it.rs`, in the existing `gating`
module — no new `[[test]]` binary is registered.

### `characterization_inv4_capacity_from_operator_config_evicts_oldest`

Exercises the operator-configuration path of the bounded inbox that the
pre-existing fixed-capacity test does not touch. Reads capacity from
`AMPLIHACK_SIGNAL_INBOX_CAPACITY` via `Inbox::default_capacity()` /
`Inbox::at_session`, using a save/restore guard around the env var so the test
never leaks state to its neighbors. It locks:

- a **valid** value (e.g. `"3"`) is honored — the Nth+1 push returns
  `PushOutcome::EvictedOldest` and the oldest instruction is gone;
- **invalid** values (`"0"`, `"-1"`, `"  "`, `"not-a-number"`) fall back to
  `Inbox::DEFAULT_CAPACITY` (32) rather than disabling the inbox, panicking, or
  creating an unbounded file;
- eviction actually **bounds** the on-disk queue (memory/DoS mitigation).

Backed by `tempfile::TempDir` — unique per-test paths, no fixed `/tmp` file, no
cross-test collision. Placement:
`crates/amplihack-signal/tests/session_channel_it.rs`.

### `characterization_inv5_successive_turns_reuse_same_session_id`

Drives `SerialTurnDriver` (bound to one pinned session id) over an injectable
`TurnRunner` mock that records the argv of every turn. Locks two things:

1. **Resume continuity** — turn *N* and turn *N+1* both carry the *same*
   `--session-id <uuid>`, so context is preserved across turns.
2. **Injection safety** — the prompt is always exactly **one** `-p` argv
   element, verbatim, even for adversarial inputs containing `;`, `&&`,
   `$(...)`, quotes, embedded newlines, and a leading `-`. It is never split or
   concatenated into a shell string.

A weak/tautological assertion here would silently allow a future
command-injection regression, so the test asserts single-argv containment with
explicit adversarial inputs. Placement:
`crates/amplihack-signal/tests/chat_it.rs`, `turn` module.

### `characterization_inv6_turn_runs_to_completion_no_wallclock_cap`

Locks that `SerialTurnDriver::run_turn` resolves with the runner's captured
output on natural completion and that the driver imposes **no** per-turn
wall-clock deadline: there is no `tokio::time::timeout` or elapsed-time cap
wrapping a single turn. Uses an injectable runner (channel-gated completion) so
the "long turn" is simulated deterministically rather than with a real sleep.
Placement: `crates/amplihack-signal/tests/chat_it.rs`, `turn` module.

### `characterization_inv7_automode_continues_until_stop_condition`

An in-crate `#[cfg(test)]` test (default build, **no** `signal` feature) inside
`auto_mode_exec.rs`. Locks the loop's stop-condition arithmetic without spawning
any agent process:

- `AutoMode::should_continue()` returns `true` while below every limit and
  `false` as soon as the **first** of `max_turns` / `max_api_calls` /
  `max_output_bytes` / `max_session_secs` is tripped;
- `AutoMode::build_execution_prompt()` emits the 1-based `[Turn n/max]` marker
  (turn index `self.turn + 1`), matching the observed current numbering.

Env/config mutation is confined to the constructed `AutoModeConfig` value, so
the test is hermetic and does not touch global process state.

## Running the suite

The added tests compile and pass under the **same** feature flags as the
existing signal tests. INV-7 builds in the default (feature-off) launcher build.

```bash
# Signal crate — INV-3/4/5/6 (feature-gated, hermetic loopback only)
cargo test -p amplihack-signal --features signal

# CLI crate — signal chat integration under the signal feature
cargo test -p amplihack-cli --features signal signal_chat_it

# Launcher crate — INV-7 in the default build (no signal feature)
cargo test -p amplihack-launcher auto_mode_exec
```

### Targeted single-test runs

```bash
cargo test -p amplihack-signal --features signal --test chat_it \
  characterization_inv3_inbound_gate_failclosed
cargo test -p amplihack-signal --features signal --test chat_it \
  characterization_inv5_successive_turns_reuse_same_session_id
cargo test -p amplihack-signal --features signal --test chat_it \
  characterization_inv6_turn_runs_to_completion_no_wallclock_cap
cargo test -p amplihack-signal --features signal --test session_channel_it \
  characterization_inv4_capacity_from_operator_config_evicts_oldest
cargo test -p amplihack-launcher \
  characterization_inv7_automode_continues_until_stop_condition
```

## Configuration reference

| Env var | Consumed by | Default | Behavior |
| ------- | ----------- | ------- | -------- |
| `AMPLIHACK_SIGNAL_INBOX_CAPACITY` | `Inbox::default_capacity()` → INV-4 | `32` (`Inbox::DEFAULT_CAPACITY`) | Positive integer sets the bounded inbox capacity. Zero, negative, non-numeric, or whitespace-only values fall back to the default (never unbounded, never disabled). |

The INV-7 stop conditions come from `AutoModeConfig`, not the environment:

| Field | Meaning |
| ----- | ------- |
| `max_turns` | Hard cap on total turns. |
| `max_api_calls` | Hard cap on SDK invocations. |
| `max_output_bytes` | Hard cap on accumulated output size. |
| `max_session_secs` | Hard cap on total wall-clock session duration (session-level, **not** per-turn — see INV-6). |

## Design constraints honored

- **No production logic changed.** The only source-file edit outside test files
  is inside the pre-existing `#[cfg(test)] mod tests` of `auto_mode_exec.rs`,
  which is excluded from non-test builds. `gating.rs`, `turn.rs`,
  `session_channel.rs`, and the loop code are untouched.
- **No new `[[test]]` entries.** INV-3/5/6 live in the already-registered
  `chat_it.rs`; INV-4 in `session_channel_it.rs`; INV-7 in the in-crate test
  module.
- **Reused seams only:** `fake_endpoint.rs`, the injectable `TurnRunner` /
  `MockRunner`, `Gate::evaluate_at` / `Gate::record_outbound_at`, `Inbox` env
  path, and `AutoMode` decision functions.
- **Feature gate preserved:** signal test files keep
  `#![cfg(feature = "signal")]`; INV-7 requires no feature.
- **Hermetic & deterministic:** no real network/process, no `sleep`-as-sync;
  channels/atomics for ordering, explicit time seams for timing, `tempfile` for
  filesystem state, and env save/restore guards to prevent cross-test leakage.

## Notes on pre-existing test flakiness (out of scope)

`cargo test -p amplihack-cli --features signal` exhibits pre-existing,
non-deterministic failures in areas this PR does not touch (multitask launcher
CRLF handling, copilot_setup staging). This PR changes zero CLI
production/test files outside `signal_chat_it`, so that environmental flakiness
is pre-existing and out of scope — it is not introduced or affected by this
characterization suite.

## Related documents

- [`docs/SIGNAL_CHAT.md`](../SIGNAL_CHAT.md) — the `/signal` chat feature this
  loop drives.
- [`docs/signal-chat-hardening.md`](../signal-chat-hardening.md) — the security
  posture (fail-closed gating, injection safety) these tests lock.
- [`docs/AUTO_MODE.md`](../AUTO_MODE.md) and
  [`docs/AUTOMODE_SAFETY.md`](../AUTOMODE_SAFETY.md) — the auto-mode loop and its
  stop conditions (INV-7).
