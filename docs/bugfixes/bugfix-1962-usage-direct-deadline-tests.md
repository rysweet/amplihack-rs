---
title: "Bug Fix #1962: load-independent deadline tests for the direct usage read"
type: explanation
issue: 1962
files:
  - crates/amplihack-cli/src/commands/profile/tests_usage_direct.rs
---

# Bug Fix #1962: load-independent deadline tests for the direct usage read

> **Issue:** [#1962](https://github.com/rysweet/amplihack-rs/issues/1962)

## Contents

- [Summary](#summary)
- [Why the old tests failed under load](#why-the-old-tests-failed-under-load)
- [The rule the tests now follow](#the-rule-the-tests-now-follow)
- [Test-local constants and helpers](#test-local-constants-and-helpers)
- [The five rewritten tests](#the-five-rewritten-tests)
- [Tests left unchanged](#tests-left-unchanged)
- [Running the tests](#running-the-tests)
- [Related documents](#related-documents)

## Summary

Five tests in `crates/amplihack-cli/src/commands/profile/tests_usage_direct.rs`
check how the direct usage read (`usage_direct.rs`) spends its single probe
deadline across a `GET`, a token refresh `POST` and a retried `GET`. They used
fake-server delays chosen a few hundred milliseconds either side of a limit.
At a host load of about 33 the scheduler stalled the read long enough to move
it across that limit, so `a_401_refresh_and_retry_share_one_deadline` saw no
refresh `POST` (`posts(&fake)` was 0, not 1). Each test passed when run alone.

The tests now take every delay and every bound from named constants: the
production constants `REFRESH_BUDGET`, `MAX_STEP` and `MIN_STEP`, and four
test-local constants. Each margin is wide enough that a scheduling stall
leaves the outcome unchanged, or moves the read further towards the branch
the test asserts.

This is a test-only change. No production code, constant or behaviour was
modified. The tests predate #1962: they were added in `a0df5d622`
(#1844/#1854). They are fixed on the #1962 branch because the full pre-push
hook runs them.

## Why the old tests failed under load

A stall in the test process adds elapsed time at whatever point it happens.
A test is fragile when added time can change which branch the code under
test takes. The old tests had three such points.

| Test | Old shape | What a stall changed |
| --- | --- | --- |
| `a_401_refresh_and_retry_share_one_deadline` | Every reply after 1.8 s, 5 s deadline. The 401 left 3.2 s against a 3 s `REFRESH_BUDGET`. | A stall of 0.2 s before the 401 arrived left less than `REFRESH_BUDGET`, so no refresh was sent and `posts == 0`. |
| `a_refresh_with_the_budget_left_saves_the_rotated_token` | 3.5 s deadline, token reply after 2.8 s. The 401 left about 3.5 s against a 3 s `REFRESH_BUDGET`. | A stall of 0.5 s before the 401 arrived meant no refresh was sent. After the send, 0.7 s of slack before the deadline; a stall turned a saved refresh into `refresh not saved`. |
| `a_refresh_with_too_little_time_left_is_not_sent` | 2 s deadline, 401 after 800 ms, elapsed bound 1.5 s. | The branch decision was safe, but the read returns about 0.8 s in, so a stall of 0.7 s broke the elapsed-time assertion. |
| `a_refresh_answered_after_the_deadline_is_not_saved` | 3.5 s deadline, token reply after 8 s, elapsed bound 3.8 s. | A stall of 0.3 s after the deadline broke the elapsed-time assertion. |
| `a_stalled_server_is_a_timeout_within_the_deadline` | 2 s deadline, reply after 4 s, elapsed bound 3.5 s. | Literal values with no stated link to the deadline; 1.5 s of slack for the read to return. |

## The rule the tests now follow

A wall-clock test in this file must satisfy both conditions below.

1. **The branch decision has a wide margin.** Any delay that decides a branch
   (refresh sent or skipped, answer in time or late) sits at least
   `SEND_MARGIN` (4 s) away from the limit, or on the side of the limit that a
   stall can only reinforce.
2. **Elapsed-time bounds are loose and derived.** An upper bound on elapsed
   time comes from the deadline, never from a hand-picked figure. A read that
   runs to its deadline is bounded by the deadline plus `RETURN_SLACK` (2 s).
   A read that should end before its deadline is bounded by the deadline
   itself, with every reply sent at once so that the whole deadline is slack.
   The result of the read, not its timing, carries the proof.

Every window that a test relies on is checked by a real `assert!` at the start
of the test. A later change to `REFRESH_BUDGET`, `MAX_STEP` or `MIN_STEP` that
closes a window fails the test with a message, rather than making it
timing-dependent again.

## Test-local constants and helpers

These items sit at the top of the deadline section of
`tests_usage_direct.rs`. `MAX_STEP` and `MIN_STEP` are imported from
`usage_direct` next to `REFRESH_BUDGET`.

| Item | Value | Meaning |
| --- | --- | --- |
| `SEND_MARGIN` | 4 s | Time a refresh decision has in hand beyond `REFRESH_BUDGET`. |
| `REFRESH_T` | `REFRESH_BUDGET + SEND_MARGIN` (7 s) | Probe deadline for the refresh tests. |
| `RETURN_SLACK` | 2 s | Time allowed for the read to return after its deadline. |
| `SHORT_T` | `REFRESH_BUDGET - 1 s` (2 s) | Probe deadline that leaves too little time to send a refresh. |
| `COUNTER_WAIT` | 30 s | How long a test waits for the process-wide `REFRESHING` count to reach zero. It is checked to be at least three times `REFRESH_T + RETURN_SLACK`, because the refresh tests run in parallel and each can hold a refresh for up to 9 s. The wait ends as soon as the count is zero. |
| `get_step(t)` | `MAX_STEP.min(t - 1 s).max(MIN_STEP)` | The connect and read step of the `GET` agent for a deadline `t`. It mirrors the formula in `usage_direct::agent`. |
| `refreshing_after(old, token, retry)` | three `Duration`s | Fake server with one delay per path: the 401 to the old token, the token reply, and the 200 to the new token. It replaces `slow_refreshing(delay)`, which used one delay for every reply. |

With the production values (`REFRESH_BUDGET` = 3 s, `MAX_STEP` = 10 s,
`MIN_STEP` = 500 ms), `get_step(REFRESH_T)` is 6 s.

`a_slow_refresh_within_the_deadline_still_reads` calls
`refreshing_after(300 ms, 300 ms, 300 ms)`, which matches its old behaviour.
It is otherwise unchanged.

### Waits on the refresh count

The longer deadlines above mean a rewritten test can hold the process-wide
`REFRESHING` count raised for about 7 s. Four other tests wait for that count
to return to zero, with limits of 5 s or 10 s, and on the first module run
they timed out while a rewritten test was still refreshing. These waits now
use `COUNTER_WAIT`:

- `a_held_lock_is_refresh_busy_and_left_alone`;
- `a_set_stop_flag_takes_no_lock_and_sends_no_post`;
- `the_refresh_counter_covers_a_refresh_in_progress`, which also asserts
  `COUNTER_WAIT >= 3 * (REFRESH_T + RETURN_SLACK)`;
- `wait_for_refreshes_returns_at_once_when_none_run`, whose elapsed-time
  bound is also `COUNTER_WAIT`.

Each wait returns as soon as the count reaches zero, so the longer limit adds
no time to a passing run. What the tests check is unchanged: a leaked count
still never reaches zero and still fails them.

## The five rewritten tests

### `a_401_refresh_and_retry_share_one_deadline`

**What it proves:** one deadline covers the whole read, and a refresh answered
in time saves the rotated tokens even when the retried `GET` then runs out of
time.

| Path | Delay | Reason |
| --- | --- | --- |
| 401 to the old token | 0 | The refresh decision is made with the full `REFRESH_T` (7 s) left, 4 s above `REFRESH_BUDGET`. |
| Token reply (`TOKEN_D`) | 2.5 s | Well inside the time the `POST` has; the rotated tokens are saved. |
| 200 to the new token (`RETRY_D`) | `get_step(REFRESH_T) - 500 ms` (5.5 s) | Fits inside one `GET` step, so a deadline per request would let it succeed. |

`TOKEN_D + RETRY_D` is 8 s, more than the 7 s deadline. Under a deadline per
request the read would return `Ok`; under the single read deadline it returns
`Err(reason::TIMEOUT)`.

Checked before the read:

- `REFRESH_T >= REFRESH_BUDGET + SEND_MARGIN`
- `TOKEN_D < REFRESH_T - REFRESH_BUDGET`
- `RETRY_D < get_step(REFRESH_T)` and `RETRY_D < REFRESH_T`
- `TOKEN_D + RETRY_D > REFRESH_T`

Checked after the read:

- the result is `Err(reason::TIMEOUT)`;
- elapsed time is below `REFRESH_T + RETURN_SLACK`;
- `posts == 1` and `gets.len() == 2`, and the retry carried
  `Bearer NEW_ACCESS`;
- `accessToken` and `refreshToken` on file equal `NEW_ACCESS` and
  `NEW_REFRESH`;
- the profile directory holds only `CREDS`.

A stall can only add time, and the test tolerates it within these limits:

- A stall before the `POST` is sent skips the refresh only if it exceeds
  `REFRESH_T - REFRESH_BUDGET` (4 s).
- A stall before the token reply pushes the retry further past the deadline,
  so the result stays `TIMEOUT`, provided the stall is under
  `REFRESH_T - TOKEN_D` (4.5 s). A longer stall delays the token reply past
  the deadline, and the result becomes `Err(reason::REFRESH_NOT_SAVED)`.

### `a_refresh_with_the_budget_left_saves_the_rotated_token`

**What it proves:** a refresh sent with the budget left is waited for beyond
the `GET` agent's step, because the `POST` gets its own agent whose step is
`left.min(MAX_STEP)`.

- Deadline `REFRESH_T` (7 s); the 401 is sent with no delay.
- `TOKEN_D = get_step(REFRESH_T) + 50 ms` (6.05 s), which is longer than a
  `GET` step and so would fail on the `GET` agent.
- Checked: `TOKEN_D > get_step(REFRESH_T)` and `TOKEN_D < MAX_STEP`.
- Assertions are unchanged: the read succeeds, `posts == 1`, the rotated
  tokens are saved and only `CREDS` remains.

The test keeps a window of about 0.95 s between the token reply and the
deadline. The production constants fix this window: the `GET` step is
`T - 1 s`, so the token reply must arrive between `T - 1 s` and `T`.
Widening it would need a production change, which is out of scope. The send
decision itself now has 4 s in hand rather than 0.5 s, and a comment in the
test records the remaining window.

### `a_refresh_answered_after_the_deadline_is_not_saved`

**What it proves:** a refresh `POST` answered after the read's deadline ends
the read at the deadline with `refresh not saved`, leaves the file untouched
and releases the lock.

- Deadline `REFRESH_T` (7 s); the 401 is sent with no delay.
- `TOKEN_D = REFRESH_T + RETURN_SLACK + 1 s` (10 s).
- Checked: `TOKEN_D > REFRESH_T + RETURN_SLACK`.
- Elapsed time is below `REFRESH_T + RETURN_SLACK`.
- Assertions are unchanged: `Err(reason::REFRESH_NOT_SAVED)`,
  `usage_source::not_saved` holds for that reason, `posts == 1`, the file is
  byte-for-byte unchanged, only `CREDS` remains and the lock file is gone.

### `a_stalled_server_is_a_timeout_within_the_deadline`

**What it proves:** a server that never answers in time gives `timeout` at
the read's deadline.

- `STALL_T` = 2 s is the read deadline.
- The reply is delayed by `STALL_T + RETURN_SLACK + 2 s` (6 s).
- Checked: the reply delay is greater than `STALL_T + RETURN_SLACK`.
- The result is `Err(reason::TIMEOUT)` and elapsed time is below
  `STALL_T + RETURN_SLACK`.

### `a_refresh_with_too_little_time_left_is_not_sent`

**What it proves:** a 401 that leaves less than `REFRESH_BUDGET` of the read
sends no refresh; the read is `timeout`, the old refresh token is never
offered to the server, the file is untouched and the lock is released.

- Deadline `SHORT_T` (2 s); the 401 is sent with no delay, in place of the
  old 800 ms.
- Checked: `SHORT_T < REFRESH_BUDGET`.
- Elapsed time is below `SHORT_T`. The read ends as soon as the 401 arrives,
  so the full 2 s is slack, against 0.7 s before.
- Assertions are unchanged: `Err(reason::TIMEOUT)`, `posts == 0`, the file
  is byte-for-byte unchanged, only `CREDS` remains and the lock file is gone.

A stall before the 401 only reduces the time left, which reinforces the
"not sent" branch.

The fake server's detached threads sleep for at most 10 s and hold only
loopback sockets.

## Tests left unchanged

- `a_stalled_lookup_gives_up_at_its_bound`: a 200 ms lookup bound against a
  2 s elapsed-time bound, a margin of about 1.8 s.
- `a_stalled_token_host_lookup_is_refresh_failed_not_refresh_not_saved`: a
  600 ms connect timeout against a 2 s elapsed-time bound, a margin of about
  1.4 s.
- `the_refresh_counter_covers_a_refresh_in_progress`: it waits up to 3 s for
  a `POST` that is sent at once. Only its final wait on the refresh count
  changed, as described in [Waits on the refresh count](#waits-on-the-refresh-count).

These margins bound elapsed time only; no branch depends on them. They are
narrower than `RETURN_SLACK`, and are left as they are because they test the
lookup bound rather than the probe deadline. If they fail under load, they
take the same treatment as the tests above.
- `crates/amplihack-cli/tests/issue_1811_governor.rs`: its load sensitivity
  is tracked in [#1977](https://github.com/rysweet/amplihack-rs/issues/1977).

## Running the tests

Run the module on its own. Test scratch goes under `$TMPDIR`; point it at a
directory whose path contains `/tmp`:

```bash
export TMPDIR="$HOME/tmp/ud-check"   # any path containing /tmp
mkdir -p "$TMPDIR"
cargo test -p amplihack-cli --lib -- tests_usage_direct::
```

The five rewritten tests take about 7 s, 7 s, 7 s, 2 s and well under 1 s
of wall time. They sleep rather than use the CPU and run in parallel with the
rest of the module, which takes about 7.5 s in all.

To check them under CPU load, confine the load to a fixed set of cores in a
scope that stops itself, then run the module repeatedly while it runs:

```bash
systemd-run --user --scope -p AllowedCPUs=56-71 -p RuntimeMaxSec=1800 -- \
  taskset -c 56-71 stress-ng --cpu 16 --timeout 1500 &

for i in 1 2 3 4 5; do
  TMPDIR="$HOME/tmp/ud-$i" cargo test -p amplihack-cli --lib -- tests_usage_direct::
done
```

Stop the scope with `systemctl --user stop <scope-unit>` when the runs end.
Do not start one load process per CPU across the whole host.

## Related documents

- [`amplihack profile` reference](../reference/profile-command.md) describes
  the `profile usage` command whose read these tests cover.
- [Bug Fix #1093](bugfix-1093-blarify-liveness-test-flake.md) fixes a
  similar timing-dependent test by a different method (a pure decision
  helper in place of real time).
