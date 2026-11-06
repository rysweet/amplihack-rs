---
title: "Bug Fix #1962: load-independent deadline tests for the direct usage read"
type: explanation
issue: 1962
files:
  - crates/amplihack-cli/src/commands/profile/tests_usage_direct.rs
---

# Bug Fix #1962: load-independent deadline tests for the direct usage read

> **Issue:** [#1962](https://github.com/rysweet/amplihack-rs/issues/1962)

This is a test-only change; no production code or constant was modified.

## Cause

Five tests in `tests_usage_direct.rs` check how the direct usage read spends
one probe deadline across a `GET`, a refresh `POST` and a retried `GET`. Their
fake-server delays sat a few hundred milliseconds from a limit. At a host load
of about 33, a scheduler stall moved the read across the limit; for example
`a_401_refresh_and_retry_share_one_deadline` saw no refresh `POST`. Each test
passed when run alone.

## The rule the tests now follow

1. A delay that decides a branch (refresh sent or skipped, answer in time or
   late) sits at least `SEND_MARGIN` (4 s) from the limit, or on the side of
   the limit that a stall can only reinforce.
2. An elapsed-time bound is derived from the deadline: the deadline plus
   `RETURN_SLACK` (2 s) for a read that runs to its deadline, or the deadline
   itself for a read that should end earlier.

Every window a test relies on is checked by an `assert!` at the start of the
test, so a later change to `REFRESH_BUDGET`, `MAX_STEP` or `MIN_STEP` that
closes a window fails with a message. The reasoning for each test's delays is
in comments beside the test.

| Item | Value | Meaning |
| --- | --- | --- |
| `SEND_MARGIN` | 4 s | Time a refresh decision has beyond `REFRESH_BUDGET`. |
| `REFRESH_T` | `REFRESH_BUDGET + SEND_MARGIN` | Probe deadline for the refresh tests. |
| `RETURN_SLACK` | 2 s | Time allowed for the read to return after its deadline. |
| `SHORT_T` | `REFRESH_BUDGET - 1 s` | Deadline too short to send a refresh. |
| `COUNTER_WAIT` | 30 s | Limit on waits for the `REFRESHING` count to reach zero; each wait ends as soon as it does. |
| `get_step(t)` | `MAX_STEP.min(t - 1 s).max(MIN_STEP)` | The `GET` agent's step, mirroring `usage_direct::agent`. |
| `refreshing_after(old, token, retry)` | three delays | Fake server with one delay per reply. |

One window cannot be widened without a production change:
`a_refresh_with_the_budget_left_saves_the_rotated_token` needs the token reply
between the `GET` step (`T - 1 s`) and the deadline `T`.

## Running the tests

Test scratch goes under `$TMPDIR`, whose path must contain `/tmp`:

```bash
export TMPDIR="$HOME/tmp/ud-check"
mkdir -p "$TMPDIR"
cargo test -p amplihack-cli --lib -- tests_usage_direct::
```

The module takes about 7.5 s; the tests sleep rather than use the CPU.

## Related documents

- [`amplihack profile` reference](../reference/profile-command.md)
- [Bug Fix #1093](bugfix-1093-blarify-liveness-test-flake.md), a similar
  timing fix by a different method.
